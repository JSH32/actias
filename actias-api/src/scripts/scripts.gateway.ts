import {
  ConnectedSocket,
  MessageBody,
  OnGatewayConnection,
  OnGatewayDisconnect,
  SubscribeMessage,
  WebSocketGateway,
  WsException,
} from '@nestjs/websockets';
import { LiveScriptDto } from './dto/livescript.dto';
import { BundleDto } from './dto/bundle.dto';
import WebSocket from 'ws';
import { IncomingMessage } from 'http';
import { ClientGrpc } from '@nestjs/microservices';
import {
  ArgumentsHost,
  Catch,
  Inject,
  Logger,
  UseFilters,
  WsExceptionFilter,
} from '@nestjs/common';
import { script_service } from 'src/protobufs/script_service';
import { lastValueFrom } from 'rxjs';
import { AuthGuard } from 'src/auth/auth.guard';
import { AuthService } from 'src/auth/auth.service';
import { Users } from 'src/entities/Users';
import { AclService } from 'src/project/acl/acl.service';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityManager, RequestContext } from '@mikro-orm/core';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { Projects } from 'src/entities/Projects';

/** Everything the gateway remembers about one connected socket. */
interface SocketData {
  user: Users;
  session?: {
    sessionId: string;
    scriptId: string;
    /**
     * The last payload stored for the session, so a ping can re-store it
     * and push the session's ttl out without the client resending files.
     */
    last: script_service.LiveScript;
  };
}

/**
 * Sends handler errors to the client as json.
 *
 * The default filter calls `client.emit`, a socket.io method plain ws
 * sockets do not have, so without this every error is invisible to the
 * client. Non-[WsException] causes are logged and reported without detail.
 */
@Catch()
export class LiveErrorsFilter implements WsExceptionFilter {
  private readonly logger = new Logger(LiveErrorsFilter.name);

  catch(exception: unknown, host: ArgumentsHost) {
    const client = host.switchToWs().getClient<WebSocket>();

    let message = 'Internal error.';
    if (exception instanceof WsException) {
      message = exception.message;
    } else {
      this.logger.error(exception);
    }

    client.send(JSON.stringify({ status: 'error', message }));
  }
}

/**
 * Gateway for live scripts.
 *
 * Guards do not run on websocket lifecycle hooks and the request object they
 * inspect does not exist for socket messages, so this gateway does its own
 * two-step: the bearer token is checked once at the http upgrade, and the
 * project acl is checked when a session starts. The `ready` reply is part of
 * the protocol: message handlers may run before the connection hook's async
 * work finishes, so clients wait for it before sending anything.
 */
// `path`, not `namespace`: namespaces are a socket.io concept, and this app
// runs the plain ws adapter, which refuses them at boot.
@UseFilters(new LiveErrorsFilter())
@WebSocketGateway({ path: '/liveScript' })
export class ScriptsGateway
  implements OnGatewayConnection, OnGatewayDisconnect
{
  private scriptService: script_service.ScriptService;
  private readonly logger = new Logger(ScriptsGateway.name);

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    private readonly authService: AuthService,
    private readonly aclService: AclService,
    private readonly em: EntityManager,
  ) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  private connectedSockets = new Map<WebSocket, SocketData>();

  handleDisconnect(@ConnectedSocket() client: WebSocket) {
    const data = this.connectedSockets.get(client);
    if (data?.session) {
      // The session itself is left to expire on its ttl rather than deleted
      // here, so a reconnect within the window resumes it.
      this.logger.log(
        `live session ${data.session.sessionId} disconnected from script ${data.session.scriptId}`,
      );
    }

    this.connectedSockets.delete(client);
  }

  /**
   * Authenticates the upgrade request; a socket that fails never enters the
   * map, so no message handler will serve it.
   */
  async handleConnection(client: WebSocket, request: IncomingMessage) {
    const token = AuthGuard.extractTokenFromHeader(request as any);
    if (!token) {
      client.close(4401, 'Authentication required');
      return;
    }

    try {
      // Websocket upgrades bypass the orm's request middleware, so every db
      // touch on this path runs inside an explicit context.
      const user = await RequestContext.createAsync(this.em, () =>
        this.authService.getUserFromToken(token),
      );
      this.connectedSockets.set(client, { user });
      client.send(JSON.stringify({ status: 'ready' }));
    } catch {
      client.close(4401, 'Authentication required');
    }
  }

  /**
   * Starts a live session: checks the caller may write the script's project,
   * stores the first bundle, and answers with the session id.
   */
  @SubscribeMessage('start')
  async handleStart(
    @ConnectedSocket() client: WebSocket,
    @MessageBody() data: LiveScriptDto,
  ) {
    const state = this.socketState(client);
    if (state.session) {
      throw new WsException('Session already started on this connection');
    }

    const script = await lastValueFrom(
      this.scriptService
        .queryScript({ id: data.scriptId })
        .pipe(toHttpException()),
    );

    const access = await RequestContext.createAsync(this.em, async () => {
      const project = await this.em.findOneOrFail(Projects, {
        id: script.projectId,
      });

      // The stored user came from the connection's orm context; the acl's
      // owner check compares identities, so the user is re-read in this one.
      const user = await this.em.findOneOrFail(Users, { id: state.user.id });

      return this.aclService.getProjectAccess(user, project, true);
    });
    if (!access.test(AccessFields.SCRIPT_RESOURCE)) {
      throw new WsException(
        'You do not have enough permissions to perform this action',
      );
    }

    const payload = this.toLiveScript(data, undefined);
    const session = await lastValueFrom(
      this.scriptService.putLiveSession(payload),
    );

    state.session = {
      sessionId: session.sessionId,
      scriptId: data.scriptId,
      last: { ...payload, sessionId: session.sessionId },
    };

    client.send(
      JSON.stringify({ status: 'created', sessionId: session.sessionId }),
    );
  }

  @SubscribeMessage('update')
  async handleUpdate(
    @ConnectedSocket() client: WebSocket,
    @MessageBody() update: LiveScriptDto,
  ) {
    const session = this.startedSession(client);

    // Both have to match. Asserting each separately keeps the socket from
    // updating a session or a script it does not own.
    const sameSession = session.sessionId === update.sessionId;
    const sameScript = session.scriptId === update.revision.scriptConfig.id;

    if (!sameSession || !sameScript) {
      throw new WsException('Invalid session or script ID passed on update');
    }

    const payload = this.toLiveScript(update, session.sessionId);
    await lastValueFrom(this.scriptService.putLiveSession(payload));
    session.last = payload;

    client.send(JSON.stringify({ status: 'updated' }));
  }

  /**
   * Keeps an idle session alive by re-storing its last payload, which pushes
   * the redis ttl out; a client with no file changes pings instead of
   * resending its bundle.
   */
  @SubscribeMessage('ping')
  async handlePing(@ConnectedSocket() client: WebSocket) {
    const session = this.startedSession(client);

    await lastValueFrom(this.scriptService.putLiveSession(session.last));

    client.send(JSON.stringify({ status: 'alive' }));
  }

  /** State for a socket that authenticated at upgrade time. */
  private socketState(client: WebSocket): SocketData {
    const state = this.connectedSockets.get(client);
    if (!state) {
      throw new WsException('Not authenticated');
    }
    return state;
  }

  /** Session for a socket that already handled `start`. */
  private startedSession(
    client: WebSocket,
  ): NonNullable<SocketData['session']> {
    const session = this.socketState(client).session;
    if (!session) {
      throw new WsException('No session initialized');
    }
    return session;
  }

  /**
   * Shapes a websocket dto into the service message. The dto arrives as
   * parsed json, not class instances, so the bundle is rehydrated before its
   * base64 file contents can become bytes.
   */
  private toLiveScript(
    data: LiveScriptDto,
    sessionId: string | undefined,
  ): script_service.LiveScript {
    return {
      sessionId,
      scriptId: data.scriptId,
      scriptConfig: data.revision.scriptConfig,
      bundle: new BundleDto(data.revision.bundle).toServiceBundle(),
    } as script_service.LiveScript;
  }
}

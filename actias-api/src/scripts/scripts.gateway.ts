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
import WebSocket from 'ws';
import { ClientGrpc } from '@nestjs/microservices';
import { Inject, Logger, UseGuards } from '@nestjs/common';
import { script_service } from 'src/protobufs/script_service';
import { lastValueFrom } from 'rxjs';
import { AuthGuard, WsAuth } from 'src/auth/auth.guard';
import { User } from 'src/auth/user.decorator';
import { Users } from 'src/entities/Users';
import { AclByFinder, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityManager } from '@mikro-orm/core';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { Projects } from 'src/entities/Projects';

interface SocketData {
  sessionId: string;
  scriptId: string;
  user: Users;
}

/**
 * Gateway for live scripts.
 */
// `path`, not `namespace`: namespaces are a socket.io concept, and this app
// runs the plain ws adapter, which refuses them at boot.
@UseGuards(AuthGuard, AclGuard)
@WebSocketGateway({ path: '/liveScript' })
export class ScriptsGateway
  implements OnGatewayConnection, OnGatewayDisconnect
{
  private scriptService: script_service.ScriptService;
  private readonly logger = new Logger(ScriptsGateway.name);

  constructor(@Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  private connectedSockets = new Map<WebSocket, SocketData>();

  handleDisconnect(@ConnectedSocket() client: WebSocket) {
    const data = this.connectedSockets.get(client);
    if (data) {
      // The session itself is left to expire on its ttl rather than deleted
      // here, so a reconnect within the window resumes it.
      this.logger.log(
        `live session ${data.sessionId} disconnected from script ${data.scriptId}`,
      );
    }

    this.connectedSockets.delete(client);
  }

  async projectFinder(request: any, em: EntityManager) {
    const script = await lastValueFrom(
      this.scriptService
        .queryScript({ id: request.params['id'] })
        .pipe(toHttpException()),
    );

    return await em.findOneOrFail(Projects, { id: script.projectId });
  }

  @AclByFinder(AccessFields.SCRIPT_RESOURCE, 'getScript')
  @WsAuth()
  async handleConnection(
    @ConnectedSocket() client: WebSocket,
    @User() user: Users,
    @MessageBody('data') data: LiveScriptDto,
  ) {
    // Initial connection shouldn't have sessionId.
    if (data.sessionId) {
      this.logger.warn(
        `rejecting connection for script ${data.scriptId}: sessionId sent on initial connect`,
      );
      client.close(4001, "Shouldn't have a sessionId on initial connect");
      return;
    }

    const session = await lastValueFrom(
      this.scriptService.putLiveSession({
        sessionId: data.sessionId,
        scriptId: data.scriptId,
        scriptConfig: data.revision.scriptConfig,
        bundle: data.revision.bundle.toServiceBundle(),
      }),
    );

    this.connectedSockets.set(client, {
      sessionId: session.sessionId,
      scriptId: data.scriptId,
      user,
    });

    client.send(
      JSON.stringify({ status: 'created', sessionId: session.sessionId }),
    );
  }

  @SubscribeMessage('update')
  async handleUpdate(
    client: WebSocket,
    @MessageBody('data') update: LiveScriptDto,
  ) {
    const data = this.connectedSockets.get(client);
    if (!data) {
      throw new WsException('No session initialized');
    }

    // Both have to match. Asserting each separately keeps the socket from
    // updating a session or a script it does not own.
    const sameSession = data.sessionId === update.sessionId;
    const sameScript = data.scriptId === update.revision.scriptConfig.id;

    if (!sameSession || !sameScript) {
      throw new WsException('Invalid session or script ID passed on update');
    }

    await lastValueFrom(
      this.scriptService.putLiveSession({
        sessionId: data.sessionId,
        scriptId: data.scriptId,
        scriptConfig: update.revision.scriptConfig,
        bundle: update.revision.bundle.toServiceBundle(),
      }),
    );

    client.send(JSON.stringify({ status: 'updated' }));
  }
}

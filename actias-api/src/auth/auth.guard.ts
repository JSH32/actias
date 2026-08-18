import {
  CanActivate,
  ExecutionContext,
  Injectable,
  SetMetadata,
  UnauthorizedException,
} from '@nestjs/common';
import { AuthService, SERVICE_TOKEN_PREFIX } from './auth.service';
import { Reflector } from '@nestjs/core';
import { WsException } from '@nestjs/websockets';

@Injectable()
export class AuthGuard implements CanActivate {
  constructor(
    private authService: AuthService,
    private readonly reflector: Reflector,
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    if (this.reflector.get<boolean>('isPublic', context.getHandler())) {
      return true;
    }

    const isWs = this.reflector.get<boolean>(
      'websocketAuth',
      context.getHandler(),
    );

    const exception = isWs
      ? new WsException('You are not authorized')
      : new UnauthorizedException();

    const request = context.switchToHttp().getRequest();
    const token = AuthGuard.extractTokenFromHeader(request);
    if (!token) throw exception;

    try {
      if (token.startsWith(SERVICE_TOKEN_PREFIX)) {
        // A machine credential: project-scoped by its ACL and never an
        // admin, so admin-only routes stay human-only.
        if (this.reflector.get<boolean>('isAdmin', context.getHandler())) {
          return false;
        }

        request['serviceToken'] = await this.authService.getServiceToken(token);
        return true;
      }

      const user = await this.authService.getUserFromToken(token);
      // Assigning the payload to the request object here
      request['user'] = user;

      // If user isn't admin and it's required then we error here.
      if (
        this.reflector.get<boolean>('isAdmin', context.getHandler()) &&
        !user.admin
      ) {
        return false;
      }
    } catch {
      throw exception;
    }

    return true;
  }

  public static extractTokenFromHeader(request: Request): string | undefined {
    const [type, token] = request.headers['authorization']?.split(' ') ?? [];
    return type === 'Bearer' ? token : undefined;
  }
}

/**
 * Set the route to be accessible without authentication on an AuthGuard'ed controller.
 */
export const Public = () => SetMetadata('isPublic', true);

/**
 * Set the route to be geared toward WebSockets.
 */
export const WsAuth = () => SetMetadata('websocketAuth', true);

/**
 * Set the route to require an admin user.
 */
export const Admin = () => SetMetadata('isAdmin', true);

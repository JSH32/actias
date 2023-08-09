import {
  CanActivate,
  ExecutionContext,
  Injectable,
  SetMetadata,
  UnauthorizedException,
} from '@nestjs/common';
import { AuthService } from './auth.service';
import { Reflector } from '@nestjs/core';

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

    const request = context.switchToHttp().getRequest();
    const token = AuthGuard.extractTokenFromHeader(request);
    if (!token) throw new UnauthorizedException();

    try {
      const user = await this.authService.getUserFromToken(token);
      // Assigning the payload to the request object here
      request['user'] = user;
    } catch {
      throw new UnauthorizedException();
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

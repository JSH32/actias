import {
  ExecutionContext,
  InternalServerErrorException,
  createParamDecorator,
} from '@nestjs/common';

/**
 * Extract user from request. Use this with {@link AuthGuard}.
 */
export const User = createParamDecorator(
  async (_: unknown, ctx: ExecutionContext) => {
    const request = ctx.switchToHttp().getRequest();

    if (!request.user) {
      throw new InternalServerErrorException(
        `User did not exist on handler. Ensure 'AuthGuard' is present on route.`,
      );
    }

    return request.user;
  },
);

/**
 * Whoever authenticated: a user session or a project service token. Use
 * with {@link AuthGuard} on routes machine credentials may call.
 */
export const Principal = createParamDecorator(
  async (_: unknown, ctx: ExecutionContext) => {
    const request = ctx.switchToHttp().getRequest();

    if (!request.user && !request.serviceToken) {
      throw new InternalServerErrorException(
        `No principal on handler. Ensure 'AuthGuard' is present on route.`,
      );
    }

    return { user: request.user, serviceToken: request.serviceToken };
  },
);

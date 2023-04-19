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

import {
  CallHandler,
  ExecutionContext,
  Injectable,
  Logger,
  NestInterceptor,
} from '@nestjs/common';
import { tap } from 'rxjs';

@Injectable()
export class AspectLogger implements NestInterceptor {
  private logger: Logger = new Logger(AspectLogger.name);

  intercept(context: ExecutionContext, next: CallHandler) {
    const req = context.switchToHttp().getRequest();
    const { statusCode } = context.switchToHttp().getResponse();
    const { originalUrl, method, params, query, body } = req;

    this.logger.log({
      originalUrl,
      method,
      params,
      query,
      body,
    });

    return next.handle().pipe(
      tap((data) =>
        this.logger.log({
          statusCode,
          data,
        }),
      ),
    );
  }
}

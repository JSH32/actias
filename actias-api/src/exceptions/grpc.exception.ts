import { HttpException, HttpStatus } from '@nestjs/common';
import { Status } from '@grpc/grpc-js/build/src/constants';
import { ServiceError } from '@grpc/grpc-js';
import { catchError, Observable, pipe, UnaryFunction } from 'rxjs';

/**
 * Convert gRPC client service error to {@link GrpcCallException}
 */
export const toHttpException = <T>(): UnaryFunction<
  Observable<T>,
  Observable<T>
> => {
  return pipe(
    catchError((err) => {
      throw new GrpcCallException(err);
    }),
  );
};

export class GrpcCallException extends HttpException {
  static statusMap = {
    [Status.OK]: HttpStatus.OK,
    [Status.CANCELLED]: HttpStatus.METHOD_NOT_ALLOWED,
    [Status.UNKNOWN]: HttpStatus.BAD_GATEWAY,
    [Status.INVALID_ARGUMENT]: HttpStatus.BAD_REQUEST,
    [Status.DEADLINE_EXCEEDED]: HttpStatus.REQUEST_TIMEOUT,
    [Status.NOT_FOUND]: HttpStatus.NOT_FOUND,
    [Status.ALREADY_EXISTS]: HttpStatus.CONFLICT,
    [Status.PERMISSION_DENIED]: HttpStatus.FORBIDDEN,
    [Status.RESOURCE_EXHAUSTED]: HttpStatus.TOO_MANY_REQUESTS,
    [Status.FAILED_PRECONDITION]: HttpStatus.PRECONDITION_REQUIRED,
    [Status.ABORTED]: HttpStatus.INTERNAL_SERVER_ERROR,
    [Status.OUT_OF_RANGE]: HttpStatus.PAYLOAD_TOO_LARGE,
    [Status.UNIMPLEMENTED]: HttpStatus.NOT_IMPLEMENTED,
    [Status.INTERNAL]: HttpStatus.INTERNAL_SERVER_ERROR,
    [Status.UNAVAILABLE]: HttpStatus.NOT_FOUND,
    [Status.DATA_LOSS]: HttpStatus.INTERNAL_SERVER_ERROR,
    [Status.UNAUTHENTICATED]: HttpStatus.UNAUTHORIZED,
  };

  constructor(grpcError: ServiceError) {
    super(grpcError.details, GrpcCallException.statusMap[grpcError.code]);
  }
}

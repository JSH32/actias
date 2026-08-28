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
    // The api is a gateway here, so a backend that did not answer in
    // time is 504, not the 408 that blames the caller for being slow.
    [Status.DEADLINE_EXCEEDED]: HttpStatus.GATEWAY_TIMEOUT,
    [Status.NOT_FOUND]: HttpStatus.NOT_FOUND,
    [Status.ALREADY_EXISTS]: HttpStatus.CONFLICT,
    [Status.PERMISSION_DENIED]: HttpStatus.FORBIDDEN,
    [Status.RESOURCE_EXHAUSTED]: HttpStatus.TOO_MANY_REQUESTS,
    [Status.FAILED_PRECONDITION]: HttpStatus.PRECONDITION_REQUIRED,
    [Status.ABORTED]: HttpStatus.INTERNAL_SERVER_ERROR,
    [Status.OUT_OF_RANGE]: HttpStatus.PAYLOAD_TOO_LARGE,
    [Status.UNIMPLEMENTED]: HttpStatus.NOT_IMPLEMENTED,
    [Status.INTERNAL]: HttpStatus.INTERNAL_SERVER_ERROR,
    // A backend that is down is not a missing resource: saying 404 here
    // told callers their project had vanished every time a service
    // restarted.
    [Status.UNAVAILABLE]: HttpStatus.SERVICE_UNAVAILABLE,
    [Status.DATA_LOSS]: HttpStatus.INTERNAL_SERVER_ERROR,
    [Status.UNAUTHENTICATED]: HttpStatus.UNAUTHORIZED,
  };

  /**
   * What to say when the backend sent no detail of its own. Only the
   * codes a caller can actually hit through a healthy deployment need
   * an entry; everything else falls back to the generic line.
   */
  static detailMap: Partial<Record<Status, string>> = {
    // Every platform service listens on the same port number, so a
    // connection to a stale address reaches a LIVE service that simply
    // does not have this method. tonic answers unimplemented with no
    // message, which used to surface as a bare 501.
    [Status.UNIMPLEMENTED]:
      'The service did not recognize this call. It is starting, or the ' +
      'address resolves to a different service.',
    [Status.UNAVAILABLE]: 'The service is unreachable.',
    [Status.DEADLINE_EXCEEDED]: 'The service did not answer in time.',
  };

  constructor(grpcError: ServiceError) {
    const status =
      GrpcCallException.statusMap[grpcError.code] ?? HttpStatus.BAD_GATEWAY;
    const detail =
      grpcError.details ||
      GrpcCallException.detailMap[grpcError.code] ||
      'The service call failed.';
    super(detail, status);
  }
}

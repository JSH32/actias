import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClientsModuleAsyncOptions, Transport } from '@nestjs/microservices';
import { join } from 'path';

export const protoBasePath = join(__dirname, '../../../protobufs');

/**
 * How every platform call behaves while the cluster moves underneath
 * it, as gRPC's own service config rather than per-call handling.
 *
 * `waitForReady` is the load-bearing one: a call issued while the
 * channel reconnects queues until it is back rather than raising
 * whatever the channel happens to be doing, so a service restart costs
 * latency instead of errors. The deadline is what keeps that from
 * becoming a hang when the service is genuinely gone. The
 * deadline is sized for the heaviest thing on this path, a publish
 * carrying its bundle to the script service over the cluster's own
 * network, and against how long a console is willing to spin before
 * admitting the platform is down.
 */
const SERVICE_CONFIG = JSON.stringify({
  methodConfig: [
    {
      // An empty name matches every method of every service.
      name: [{}],
      waitForReady: true,
      timeout: '15s',
      retryPolicy: {
        maxAttempts: 4,
        initialBackoff: '0.2s',
        maxBackoff: '2s',
        backoffMultiplier: 2,
        // Unavailable alone: the rest either succeeded, will fail the
        // same way again, or are the service's considered answer.
        retryableStatusCodes: ['UNAVAILABLE'],
      },
    },
  ],
});

/**
 * Channel options shared by every platform client.
 *
 * The keepalive and re-resolution numbers matter because all of the
 * services listen on one port number: a connection held to an address
 * whose container has been replaced can reach a different live
 * service, which answers unimplemented rather than refusing. Noticing
 * a dead peer quickly and re-resolving its name keeps that window
 * short.
 */
const CHANNEL_OPTIONS = {
  'grpc.service_config': SERVICE_CONFIG,
  'grpc.enable_retries': 1,
  'grpc.keepalive_time_ms': 20000,
  'grpc.keepalive_timeout_ms': 5000,
  'grpc.keepalive_permit_without_calls': 1,
  'grpc.dns_min_time_between_resolutions_ms': 1000,
};

export const grpcClient = (
  name: string,
  packageName: string,
  protoPaths: string[],
  configValue: string,
): ClientsModuleAsyncOptions => [
  {
    name: name,
    imports: [ConfigModule],
    inject: [ConfigService],
    useFactory: async (configService: ConfigService) => ({
      transport: Transport.GRPC,
      options: {
        url: configService.get<string>(configValue),
        maxReceiveMessageLength: Number.MAX_SAFE_INTEGER,
        package: packageName,
        protoPath: protoPaths,
        channelOptions: CHANNEL_OPTIONS,
      },
    }),
  },
];

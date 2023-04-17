import { ConfigModule, ConfigService } from '@nestjs/config';
import { ClientsModuleAsyncOptions, Transport } from '@nestjs/microservices';
import { join } from 'path';

export const protoBasePath = join(__dirname, '../../../protobufs');

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
      },
    }),
  },
];

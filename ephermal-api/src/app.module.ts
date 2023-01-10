import { Module } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import {
  ClientsModule,
  ClientsModuleAsyncOptions,
  Transport,
} from '@nestjs/microservices';
import { join } from 'path';
import config from './config';
import { ScriptsController } from './scripts/scripts.controller';
import { RevisionsController } from './revisions/revisions.controller';

const protoBasePath = join(__dirname, '../../protobufs');

const grpcClient = (
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
        package: packageName,
        protoPath: protoPaths,
      },
    }),
  },
];

@Module({
  imports: [
    ConfigModule.forRoot({
      load: [config],
    }),
    ClientsModule.registerAsync(
      grpcClient(
        'SCRIPT_SERVICE',
        'script_service',
        [
          'google/protobuf/empty.proto',
          `${protoBasePath}/shared/bundle.proto`,
          `${protoBasePath}/script_service.proto`,
        ],
        'externalServices.scriptServiceUri',
      ),
    ),
  ],
  controllers: [ScriptsController, RevisionsController],
  providers: [],
})
export class AppModule {}

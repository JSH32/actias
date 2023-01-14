import { Module } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { RevisionsController } from './revisions.controller';
import { ScriptsController } from './scripts.controller';

@Module({
  imports: [
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
})
export class ScriptModule {}

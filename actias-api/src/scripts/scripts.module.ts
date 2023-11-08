import { Module, forwardRef } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { RevisionsController } from './revisions.controller';
import {
  ProjectScriptController,
  ScriptsController,
} from './scripts.controller';
import { AclModule } from 'src/project/acl/acl.module';
import { AuthModule } from 'src/auth/auth.module';
import { ProjectModule } from 'src/project/project.module';
import { ScriptsGateway } from './scripts.gateway';

@Module({
  imports: [
    AuthModule,
    AclModule,
    forwardRef(() => ProjectModule),
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
  exports: [ClientsModule],
  controllers: [
    ScriptsController,
    ProjectScriptController,
    RevisionsController,
  ],
  providers: [ScriptsGateway],
})
export class ScriptModule {}

import { Module } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { RevisionsController } from './revisions.controller';
import {
  ProjectScriptController,
  ScriptsController,
} from './scripts.controller';
import { AclModule } from 'src/project/acl/acl.module';
import { AuthModule } from 'src/auth/auth.module';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Projects } from 'src/entities/Projects';
import { Resources } from 'src/entities/Resources';
import { ProjectModule } from 'src/project/project.module';
import { ScriptSubscriber } from './scripts.subscriber';

@Module({
  imports: [
    AuthModule,
    AclModule,
    ProjectModule,
    MikroOrmModule.forFeature([Projects, Resources]),
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
  providers: [ScriptSubscriber],
  controllers: [
    ScriptsController,
    ProjectScriptController,
    RevisionsController,
  ],
})
export class ScriptModule {}

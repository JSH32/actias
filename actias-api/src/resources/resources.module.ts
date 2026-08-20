import { Module, forwardRef } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { AclModule } from 'src/project/acl/acl.module';
import { AuthModule } from 'src/auth/auth.module';
import { ProjectModule } from 'src/project/project.module';
import { ScriptModule } from 'src/scripts/scripts.module';
import { ResourcesController } from './resources.controller';

@Module({
  imports: [
    AuthModule,
    AclModule,
    forwardRef(() => ProjectModule),
    ScriptModule,
    ClientsModule.registerAsync(
      grpcClient(
        'NODE_REGISTRY',
        'node_registry',
        ['google/protobuf/empty.proto', `${protoBasePath}/node_registry.proto`],
        // The registry lives inside the script service binary.
        'externalServices.scriptServiceUri',
      ),
    ),
  ],
  controllers: [ResourcesController],
})
export class ResourcesModule {}

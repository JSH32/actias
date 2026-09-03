import { Module, forwardRef } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { AclModule } from 'src/project/acl/acl.module';
import { AuthModule } from 'src/auth/auth.module';
import { ProjectModule } from 'src/project/project.module';
import { ScriptModule } from 'src/scripts/scripts.module';
import { KvModule } from 'src/kv/kv.module';
import { ResourcesService } from './resources.service';
import { QueuesController } from './queues.controller';
import { DatabasesController } from './databases.controller';
import { ObjectsController } from './objects.controller';
import { WorkflowsController } from './workflows.controller';
import { ShellController } from './shell.controller';
import { ConnectionsController } from './connections.controller';

@Module({
  imports: [
    AuthModule,
    AclModule,
    forwardRef(() => ProjectModule),
    ScriptModule,
    // The shell derives its kv grants from the project's namespaces.
    KvModule,
    ClientsModule.registerAsync(
      grpcClient(
        'NODE_REGISTRY',
        'node_registry',
        ['google/protobuf/empty.proto', `${protoBasePath}/node_registry.proto`],
        // The registry lives inside the script service binary.
        'externalServices.scriptServiceUri',
      ),
    ),
    ClientsModule.registerAsync(
      grpcClient(
        'WORKER_DATA',
        'worker_data',
        [`${protoBasePath}/worker_data.proto`],
        // Any worker node's data plane; calls forward to the holder.
        'worker.grpcUrl',
      ),
    ),
  ],
  controllers: [
    QueuesController,
    DatabasesController,
    ObjectsController,
    WorkflowsController,
    ShellController,
    ConnectionsController,
  ],
  providers: [ResourcesService],
})
export class ResourcesModule {}

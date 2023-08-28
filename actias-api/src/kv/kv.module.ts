import { Module, forwardRef } from '@nestjs/common';
import { KvController } from './kv.controller';
import { ClientsModule } from '@nestjs/microservices';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { AuthModule } from 'src/auth/auth.module';
import { AclModule } from 'src/project/acl/acl.module';
import { ProjectModule } from 'src/project/project.module';

@Module({
  imports: [
    AuthModule,
    AclModule,
    forwardRef(() => ProjectModule),
    ClientsModule.registerAsync(
      grpcClient(
        'KV_SERVICE',
        'kv_service',
        ['google/protobuf/empty.proto', `${protoBasePath}/kv_service.proto`],
        'externalServices.kvServiceUri',
      ),
    ),
  ],
  exports: [ClientsModule],
  controllers: [KvController],
})
export class KvModule {}

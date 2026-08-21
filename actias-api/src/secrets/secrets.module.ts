import { Module, forwardRef } from '@nestjs/common';
import { ClientsModule } from '@nestjs/microservices';
import { AuthModule } from 'src/auth/auth.module';
import { AclModule } from 'src/project/acl/acl.module';
import { ProjectModule } from 'src/project/project.module';
import { grpcClient, protoBasePath } from 'src/util/grpc';
import { SecretsController } from './secrets.controller';

@Module({
  imports: [
    AuthModule,
    AclModule,
    forwardRef(() => ProjectModule),
    ClientsModule.registerAsync(
      grpcClient(
        'SECRET_SERVICE',
        'secret_service',
        [
          'google/protobuf/empty.proto',
          `${protoBasePath}/secret_service.proto`,
        ],
        'externalServices.secretServiceUri',
      ),
    ),
  ],
  controllers: [SecretsController],
})
export class SecretsModule {}

import { Module, forwardRef } from '@nestjs/common';
import { ProjectController } from './project.controller';
import { ServiceTokensController } from './service-tokens.controller';
import { AclModule } from './acl/acl.module';
import { ProjectService } from './project.service';
import { AuthModule } from 'src/auth/auth.module';
import { UsersModule } from 'src/users/users.module';
import { ProjectSubscriber } from './project.subscriber';
import { ScriptModule } from 'src/scripts/scripts.module';
import { KvModule } from 'src/kv/kv.module';

@Module({
  controllers: [ProjectController, ServiceTokensController],
  imports: [
    AclModule,
    AuthModule,
    UsersModule,
    // Needed for project subscriber.
    forwardRef(() => ScriptModule),
    forwardRef(() => KvModule),
  ],
  exports: [ProjectService],
  providers: [ProjectService, ProjectSubscriber],
})
export class ProjectModule {}

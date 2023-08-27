import { Module, forwardRef } from '@nestjs/common';
import { ProjectController } from './project.controller';
import { AclModule } from './acl/acl.module';
import { ProjectService } from './project.service';
import { AuthModule } from 'src/auth/auth.module';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Projects } from 'src/entities/Projects';
import { UsersModule } from 'src/users/users.module';
import { ProjectSubscriber } from './project.subscriber';
import { ScriptModule } from 'src/scripts/scripts.module';

@Module({
  controllers: [ProjectController],
  imports: [
    AclModule,
    AuthModule,
    UsersModule,
    forwardRef(() => ScriptModule),
    MikroOrmModule.forFeature([Projects]),
  ],
  exports: [ProjectService],
  providers: [ProjectService, ProjectSubscriber],
})
export class ProjectModule {}

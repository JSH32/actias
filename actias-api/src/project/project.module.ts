import { Module } from '@nestjs/common';
import { ProjectController } from './project.controller';
import { AclModule } from './acl/acl.module';
import { ProjectService } from './project.service';
import { AuthModule } from 'src/auth/auth.module';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Projects } from 'src/entities/Projects';
import { UsersModule } from 'src/users/users.module';
import { Resources } from 'src/entities/Resources';

@Module({
  controllers: [ProjectController],
  imports: [
    AclModule,
    AuthModule,
    UsersModule,
    MikroOrmModule.forFeature([Projects, Resources]),
  ],
  exports: [ProjectService],
  providers: [ProjectService],
})
export class ProjectModule {}

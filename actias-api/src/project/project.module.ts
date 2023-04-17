import { Module, UseGuards } from '@nestjs/common';
import { ProjectController } from './project.controller';
import { AclModule } from './acl/acl.module';
import { APP_GUARD } from '@nestjs/core';
import { AuthGuard } from '../auth/auth.guard';
import { ProjectService } from './project.service';
import { AclGuard } from './acl/acl.guard';
import { AuthModule } from 'src/auth/auth.module';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Projects } from 'src/entities/Projects';
import { UsersModule } from 'src/users/users.module';

@UseGuards(AuthGuard)
@Module({
  controllers: [ProjectController],
  imports: [
    AclModule,
    AuthModule,
    UsersModule,
    MikroOrmModule.forFeature([Projects]),
  ],
  providers: [
    {
      provide: APP_GUARD,
      useClass: AclGuard,
    },
    ProjectService,
  ],
})
export class ProjectModule {}

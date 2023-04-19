import { Module } from '@nestjs/common';
import { AclService } from './acl.service';
import { AclController, AclInfoController } from './acl.controller';
import { AuthModule } from 'src/auth/auth.module';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';

@Module({
  controllers: [AclController, AclInfoController],
  providers: [AclService],
  exports: [AclService],
  imports: [AuthModule, MikroOrmModule.forFeature([Projects, Users])],
})
export class AclModule {}

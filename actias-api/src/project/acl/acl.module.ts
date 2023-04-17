import { Module } from '@nestjs/common';
import { AclService } from './acl.service';
import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Access } from 'src/entities/Access';

@Module({
  providers: [AclService],
  exports: [AclService],
  imports: [MikroOrmModule.forFeature([Access])],
})
export class AclModule {}

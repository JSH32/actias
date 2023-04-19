import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Module } from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { UsersController } from './users.controller';
import { UsersService } from './users.service';
import { UserAuthMethod } from 'src/entities/UserAuthMethod';

@Module({
  controllers: [UsersController],
  providers: [UsersService],
  exports: [UsersService],
  imports: [MikroOrmModule.forFeature([Users, UserAuthMethod])],
})
export class UsersModule {}

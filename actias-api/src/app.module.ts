import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Module } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import config from './config';
import { ScriptModule } from './scripts/scripts.module';
import { UsersModule } from './users/users.module';
import { AclModule } from './acl/acl.module';

@Module({
  imports: [
    ConfigModule.forRoot({
      load: [config],
    }),
    MikroOrmModule.forRoot(),
    ScriptModule,
    UsersModule,
    AclModule,
  ],
  controllers: [],
  providers: [],
})
export class AppModule {}

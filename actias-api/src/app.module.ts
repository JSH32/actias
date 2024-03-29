import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Module } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import config from './config';
import { UsersModule } from './users/users.module';
import { AuthModule } from './auth/auth.module';
import { ProjectModule } from './project/project.module';
import { ScriptModule } from './scripts/scripts.module';
import { AspectLogger } from './util/aspectlogger';
import { APP_INTERCEPTOR } from '@nestjs/core';
import { KvModule } from './kv/kv.module';
import { AdminModule } from './admin/admin.module';

@Module({
  imports: [
    ScriptModule,
    ConfigModule.forRoot({
      isGlobal: true,
      load: [config],
    }),
    MikroOrmModule.forRoot(),
    UsersModule,
    AuthModule,
    ProjectModule,
    KvModule,
    AdminModule,
  ],
  controllers: [],
  providers: [
    {
      provide: APP_INTERCEPTOR,
      useClass: AspectLogger,
    },
  ],
})
export class AppModule {}

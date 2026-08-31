import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Module } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import config from './config';
import { UsersModule } from './users/users.module';
import { AuthModule } from './auth/auth.module';
import { ProjectModule } from './project/project.module';
import { HealthModule } from './health/health.module';
import { ScriptModule } from './scripts/scripts.module';
import { AspectLogger } from './util/aspectlogger';
import { APP_INTERCEPTOR } from '@nestjs/core';
import { ResourcesModule } from './resources/resources.module';
import { KvModule } from './kv/kv.module';
import { SecretsModule } from './secrets/secrets.module';
import { AdminModule } from './admin/admin.module';
import { InterestModule } from './interest/interest.module';

@Module({
  imports: [
    HealthModule,
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
    SecretsModule,
    AdminModule,
    ResourcesModule,
    InterestModule,
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

import { MikroOrmModule } from '@mikro-orm/nestjs';
import { Module, Logger } from '@nestjs/common';
import { ConfigModule, ConfigService } from '@nestjs/config';
import config from './config';
import { ScriptModule } from './scripts/scripts.module';
import { UsersModule } from './users/users.module';
import { AclModule } from './acl/acl.module';

@Module({
  imports: [
    ConfigModule.forRoot({
      load: [config],
    }),
    MikroOrmModule.forRootAsync({
      imports: [ConfigModule],
      inject: [ConfigService],
      useFactory: async (configService: ConfigService) => {
        const logger = new Logger('MikroORM');
        return {
          entities: ['./dist/entities'],
          entitiesTs: ['./src/entities'],
          clientUrl: configService.get('databaseUrl'),
          type: 'postgresql',
          logger: logger.log.bind(logger),
          driverOptions: {
            // Otherwise this fails in knex since it can't get the version.
            version: 'CockroachDB',
          },
        };
      },
    }),
    ScriptModule,
    UsersModule,
    AclModule,
  ],
  controllers: [],
  providers: [],
})
export class AppModule {}

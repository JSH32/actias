import { defineConfig } from '@mikro-orm/postgresql';
import { TsMorphMetadataProvider } from '@mikro-orm/reflection';
import { Logger } from '@nestjs/common';
import config from './config';

const logger = new Logger('MikroORM');

// Config for CLI.
export default defineConfig({
  type: 'postgresql',
  metadataProvider: TsMorphMetadataProvider,
  entities: ['./dist/entities'],
  entitiesTs: ['./src/entities'],
  clientUrl: config().databaseUrl,
  logger: logger.log.bind(logger),
  driverOptions: {
    // Otherwise this fails in knex since it can't get the version.
    version: 'CockroachDB',
  },
  migrations: {
    disableForeignKeys: false,
  },
});

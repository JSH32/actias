import { defineConfig } from '@mikro-orm/postgresql';
import { TsMorphMetadataProvider } from '@mikro-orm/reflection';
import { BadRequestException, Logger } from '@nestjs/common';
import config from './config';

const logger = new Logger('MikroORM');

// Config for CLI.
export default defineConfig({
  debug: true,
  type: 'postgresql',
  metadataProvider: TsMorphMetadataProvider,
  entities: ['./dist/entities'],
  entitiesTs: ['./src/entities'],
  clientUrl: config().databaseUrl,
  // Reads go to the replica when one is configured; a request that
  // wrote keeps reading the primary within its own unit of work.
  ...(config().readDatabaseUrl
    ? { replicas: [{ clientUrl: config().readDatabaseUrl }] }
    : {}),
  logger: logger.log.bind(logger),
  migrations: {
    disableForeignKeys: false,
  },
  findOneOrFailHandler: (entityName) =>
    new BadRequestException(`${entityName} was not found.`),
});

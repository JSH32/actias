import { defineConfig } from '@mikro-orm/mongodb';
import { TsMorphMetadataProvider } from '@mikro-orm/reflection';
import { Logger, NotFoundException } from '@nestjs/common';
import config from './config';

const logger = new Logger('MikroORM');

// Config for CLI.
export default defineConfig({
  debug: true,
  type: 'mongo',
  dbName: 'atlasapidb',
  metadataProvider: TsMorphMetadataProvider,
  entities: ['./dist/entities'],
  entitiesTs: ['./src/entities'],
  clientUrl: config().databaseUrl,
  logger: logger.log.bind(logger),
  implicitTransactions: false,
  ensureIndexes: true,
  findOneOrFailHandler: (entityName, where) => {
    throw new NotFoundException(
      `${entityName} was not found${
        where['id'] != null ? ` with that ID (${where['id']})` : ''
      }`,
    );
  },
});

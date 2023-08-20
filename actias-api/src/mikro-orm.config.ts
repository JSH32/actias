import { defineConfig } from '@mikro-orm/postgresql';
import { TsMorphMetadataProvider } from '@mikro-orm/reflection';
import { BadRequestException, Logger } from '@nestjs/common';
import config from './config';
import { ScriptSubscriber } from './scripts/scripts.subscriber';

const logger = new Logger('MikroORM');

// Config for CLI.
export default defineConfig({
  debug: true,
  type: 'postgresql',
  metadataProvider: TsMorphMetadataProvider,
  entities: ['./dist/entities'],
  entitiesTs: ['./src/entities'],
  clientUrl: config().databaseUrl,
  logger: logger.log.bind(logger),
  migrations: {
    disableForeignKeys: false,
  },
  findOneOrFailHandler: (entityName) =>
    new BadRequestException(`${entityName} was not found.`),
});

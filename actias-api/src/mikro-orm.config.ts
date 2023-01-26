import { defineConfig } from '@mikro-orm/postgresql';
import * as dotenv from 'dotenv';
dotenv.config();

// Config for CLI.
export default defineConfig({
  type: 'postgresql',
  clientUrl: process.env.DATABASE_URL,
  driverOptions: {
    // Otherwise this fails in knex since it can't get the version.
    version: 'CockroachDB',
  },
});

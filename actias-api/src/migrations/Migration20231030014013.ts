import { Migration } from '@mikro-orm/migrations';

export class Migration20231030014013 extends Migration {
  async up(): Promise<void> {
    this.addSql(
      'create table "registration_codes" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "uses" int not null default 1, constraint "registration_codes_pkey" primary key ("id"));',
    );
  }

  async down(): Promise<void> {
    this.addSql('drop table if exists "registration_codes" cascade;');
  }
}

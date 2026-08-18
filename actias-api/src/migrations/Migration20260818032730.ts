import { Migration } from '@mikro-orm/migrations';

export class Migration20260818032730 extends Migration {
  async up(): Promise<void> {
    this.addSql(
      'create table "service_tokens" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "name" varchar(64) not null, "token_hash" varchar(64) not null, "token_prefix" varchar(16) not null, "project_id" uuid not null, "permission_bitfield" varchar(255) not null, "last_used" timestamptz(0) null, constraint "service_tokens_pkey" primary key ("id"));',
    );
    this.addSql(
      'alter table "service_tokens" add constraint "service_tokens_token_hash_unique" unique ("token_hash");',
    );

    this.addSql(
      'alter table "service_tokens" add constraint "service_tokens_project_id_foreign" foreign key ("project_id") references "projects" ("id") on update cascade on delete cascade;',
    );
  }

  async down(): Promise<void> {
    this.addSql('drop table if exists "service_tokens" cascade;');
  }
}

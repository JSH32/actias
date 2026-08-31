import { Migration } from '@mikro-orm/migrations';

export class Migration20260831035531 extends Migration {

  async up(): Promise<void> {
    this.addSql('create table "interest_signup" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "email" varchar(255) not null, "source" varchar(255) not null, constraint "interest_signup_pkey" primary key ("id"));');
    this.addSql('alter table "interest_signup" add constraint "interest_signup_email_unique" unique ("email");');
  }

  async down(): Promise<void> {
    this.addSql('drop table if exists "interest_signup" cascade;');
  }

}

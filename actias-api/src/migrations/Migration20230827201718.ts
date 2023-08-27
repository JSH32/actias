import { Migration } from '@mikro-orm/migrations';

export class Migration20230827201718 extends Migration {

  async up(): Promise<void> {
    this.addSql('create table "users" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "email" varchar(320) not null, "username" varchar(36) not null, "admin" boolean not null default false, constraint "users_pkey" primary key ("id"));');
    this.addSql('alter table "users" add constraint "users_email_unique" unique ("email");');
    this.addSql('alter table "users" add constraint "users_username_unique" unique ("username");');

    this.addSql('create table "user_auth_method" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "cached_username" varchar(255) null, "method" text check ("method" in (\'password\', \'google\', \'github\')) not null, "value" varchar(255) not null, "user_id" uuid not null, constraint "user_auth_method_pkey" primary key ("id"));');
    this.addSql('alter table "user_auth_method" add constraint "user_auth_method_user_id_method_unique" unique ("user_id", "method");');

    this.addSql('create table "projects" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "name" varchar(32) not null, "owner_id" uuid not null, "default_permissions" int not null default 110, constraint "projects_pkey" primary key ("id"));');

    this.addSql('create table "access" ("id" uuid not null default gen_random_uuid(), "created_at" timestamptz(0) not null default now(), "updated_at" timestamptz(0) not null default now(), "user_id" uuid not null, "project_id" uuid not null, "permission_bitfield" varchar(255) not null, constraint "access_pkey" primary key ("id"));');
    this.addSql('alter table "access" add constraint "access_user_id_project_id_unique" unique ("user_id", "project_id");');

    this.addSql('alter table "user_auth_method" add constraint "user_auth_method_user_id_foreign" foreign key ("user_id") references "users" ("id") on update cascade on delete cascade;');

    this.addSql('alter table "projects" add constraint "projects_owner_id_foreign" foreign key ("owner_id") references "users" ("id") on update cascade on delete cascade;');

    this.addSql('alter table "access" add constraint "access_user_id_foreign" foreign key ("user_id") references "users" ("id") on update cascade on delete cascade;');
    this.addSql('alter table "access" add constraint "access_project_id_foreign" foreign key ("project_id") references "projects" ("id") on update cascade on delete cascade;');
  }

}

import { Migration } from '@mikro-orm/migrations';

export class Migration20230417192732 extends Migration {
  async up(): Promise<void> {
    this.addSql(
      'alter table "projects" alter column "default_permissions" type int using ("default_permissions"::int);',
    );
    this.addSql(
      'alter table "projects" alter column "default_permissions" set default 110;',
    );
  }

  async down(): Promise<void> {
    this.addSql(
      'alter table "projects" alter column "default_permissions" drop default;',
    );
    this.addSql(
      'alter table "projects" alter column "default_permissions" type int using ("default_permissions"::int);',
    );
  }
}

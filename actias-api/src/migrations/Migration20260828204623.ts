import { Migration } from '@mikro-orm/migrations';

export class Migration20260828204623 extends Migration {
  async up(): Promise<void> {
    this.addSql('alter table "projects" drop column "default_permissions";');
  }

  async down(): Promise<void> {
    this.addSql(
      'alter table "projects" add column "default_permissions" int not null default 110;',
    );
  }
}

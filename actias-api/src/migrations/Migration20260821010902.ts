import { Migration } from '@mikro-orm/migrations';

/**
 * The SECRETS_READ (1 << 9) and SECRETS_WRITE (1 << 10) bits arrive, and
 * secrets endpoints move off the PERMISSIONS_* bits they borrowed.
 * Capability-preserving: whoever could see or manage secrets yesterday
 * (via PERMISSIONS_READ = 1 << 3 / PERMISSIONS_WRITE = 1 << 4) can today,
 * for members and service tokens alike.
 *
 * Bitfields are stored as easy-bits binary digit strings ('100010', '0'),
 * hence the lpad/bit(64) dance in and the ltrim back out.
 */
export class Migration20260821010902 extends Migration {
  private rewrite(table: string, expression: string): void {
    this.addSql(
      `update "${table}" set "permission_bitfield" = (` +
        `select case when nv = 0 then '0' else ltrim(nv::bit(64)::text, '0') end from (` +
        `select ${expression} as nv` +
        `) s);`,
    );
  }

  /** The row's bitfield as a bigint. */
  private value(table: string): string {
    return `(lpad("${table}"."permission_bitfield", 64, '0')::bit(64)::bigint)`;
  }

  async up(): Promise<void> {
    for (const table of ['access', 'service_tokens']) {
      const v = this.value(table);
      this.rewrite(
        table,
        `${v} | (case when (${v} & 8) != 0 then 512 else 0 end)` +
          ` | (case when (${v} & 16) != 0 then 1024 else 0 end)`,
      );
    }
  }

  async down(): Promise<void> {
    for (const table of ['access', 'service_tokens']) {
      this.rewrite(table, `${this.value(table)} & ~1536`);
    }
  }
}

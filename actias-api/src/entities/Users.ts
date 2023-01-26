import {
  Entity,
  OptionalProps,
  PrimaryKey,
  Property,
  Unique,
} from '@mikro-orm/core';

@Entity()
export class Users {
  [OptionalProps]?: 'admin' | 'created' | 'id';

  @Unique({ name: 'users_id_key' })
  @PrimaryKey({ columnType: 'uuid', defaultRaw: `gen_random_uuid()` })
  id!: string;

  @Property({ length: 6, defaultRaw: `now():::TIMESTAMPTZ` })
  created!: Date;

  @Unique({ name: 'users_email_key' })
  @Property({ length: 320 })
  email!: string;

  @Property({ default: false })
  admin = false;
}

import {
  Entity,
  OptionalProps,
  PrimaryKey,
  Property,
  Unique,
} from '@mikro-orm/core';

@Entity()
@Unique({
  name: 'user_auth_methods_auth_method_user_id_key',
  properties: ['authMethod', 'userId'],
})
export class UserAuthMethods {
  [OptionalProps]?: 'id' | 'lastAccessed';

  @Unique({ name: 'user_auth_methods_id_key' })
  @PrimaryKey({ columnType: 'uuid', defaultRaw: `gen_random_uuid()` })
  id!: string;

  @Property({ columnType: 'uuid' })
  userId!: string;

  @Property({ columnType: 'text' })
  cachedUsername!: string;

  @Property({ columnType: 'auth_method' })
  authMethod!: unknown;

  @Property({ columnType: 'text' })
  value!: string;

  @Property({ length: 6, defaultRaw: `now():::TIMESTAMPTZ` })
  lastAccessed!: Date;
}

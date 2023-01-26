import {
  Entity,
  OptionalProps,
  PrimaryKey,
  Property,
  Unique,
} from '@mikro-orm/core';

@Entity()
export class Projects {
  [OptionalProps]?: 'id';

  @Unique({ name: 'projects_id_key' })
  @PrimaryKey({ columnType: 'uuid', defaultRaw: `gen_random_uuid()` })
  id!: string;

  @Property({ length: 32 })
  name!: string;

  @Property({ columnType: 'uuid' })
  ownerId!: string;
}

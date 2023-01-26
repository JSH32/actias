import {
  Entity,
  OneToOne,
  OptionalProps,
  PrimaryKey,
  Property,
  Unique,
} from '@mikro-orm/core';
import { Projects } from './Projects';

@Entity()
export class DefaultPermissions {
  [OptionalProps]?: 'id';

  @Unique({ name: 'default_permissions_id_key' })
  @PrimaryKey({ columnType: 'uuid', defaultRaw: `gen_random_uuid()` })
  id!: string;

  @OneToOne({
    entity: () => Projects,
    onDelete: 'cascade',
    unique: 'default_permissions_project_id_key',
  })
  project!: Projects;

  @Property({ columnType: 'int2' })
  scriptBitfield!: number;
}

import { Entity, ManyToOne, PrimaryKey } from '@mikro-orm/core';
import { Projects } from './Projects';
import { Users } from './Users';

@Entity()
export class Access {
  @ManyToOne({ entity: () => Users, onDelete: 'cascade', primary: true })
  user!: Users;

  @ManyToOne({ entity: () => Projects, onDelete: 'cascade', primary: true })
  project!: Projects;

  @PrimaryKey({ columnType: 'resource_type' })
  resourceType!: unknown;

  @PrimaryKey({ columnType: 'int2' })
  permissionBitfield!: number;
}

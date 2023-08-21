import { Entity, ManyToOne, Property, Unique } from '@mikro-orm/core';
import { Users } from './Users';
import { Projects } from './Projects';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * Access control per user owned by a project.
 */
@Entity()
@Unique({ properties: ['user', 'project'] })
export class Access extends ActiasBaseEntity {
  @ManyToOne({ onDelete: 'cascade' })
  user!: Users;

  @ManyToOne({ onDelete: 'cascade' })
  project!: Projects;

  @Property()
  permissionBitfield!: string;

  constructor(access: Partial<Access>) {
    super();
    Object.assign(this, access);
  }
}

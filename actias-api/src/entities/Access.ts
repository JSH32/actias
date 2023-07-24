import { Cascade, Entity, ManyToOne, Property, Unique } from '@mikro-orm/core';
import { Users } from './Users';
import { Projects } from './Projects';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * Access control per user owned by a project.
 */
@Entity()
@Unique({ properties: ['user', 'project'] })
export class Access extends ActiasBaseEntity {
  @ManyToOne({ cascade: [Cascade.REMOVE] })
  user!: Users;

  @ManyToOne({ cascade: [Cascade.REMOVE] })
  project!: Projects;

  @Property()
  permissionBitfield!: number;

  constructor(access: Required<Access>) {
    super();
    Object.assign(this, access);
  }
}

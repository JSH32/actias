import { Cascade, Entity, ManyToOne, Property, Unique } from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Projects } from './Projects';
import { Users } from './Users';

/**
 * Access control table per user per resource type.
 * There should be only one access record per user per project.
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
}

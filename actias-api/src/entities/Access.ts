import { Entity, OneToOne, Unique } from '@mikro-orm/core';
import { BaseEntity } from './BaseEntity';
import { Projects } from './Projects';
import { Users } from './Users';

@Entity()
@Unique({ properties: ['user', 'project'] })
export class Access extends BaseEntity {
  @OneToOne()
  user!: Users;

  @OneToOne()
  project!: Projects;

  permissionBitfield!: number;
}

import {
  Collection,
  Entity,
  ManyToOne,
  OneToMany,
  Property,
} from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Access } from './Access';
import { Users } from './Users';

/**
 * Projects which contain grouped resources.
 */
@Entity()
export class Projects extends ActiasBaseEntity {
  /**
   * Project name.
   */
  @Property({ length: 32 })
  name!: string;

  /**
   * Single owner of a project.
   */
  @ManyToOne({ onDelete: 'cascade' })
  owner!: Users;

  @OneToMany(() => Access, (access) => access.project, {
    orphanRemoval: true,
  })
  access = new Collection<Access>(this);

  constructor(data: Partial<Projects>) {
    super();
    Object.assign(this, data);
  }
}

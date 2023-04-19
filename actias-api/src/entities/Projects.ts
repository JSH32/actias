import {
  Collection,
  Embedded,
  Entity,
  OneToMany,
  Property,
} from '@mikro-orm/core';
import { Resources } from './Resources';
import { ActiasBaseEntity } from './BaseEntity';
import { Access } from './Access';

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
  @Property()
  ownerId!: string;

  /**
   * All resources owned by the project.
   */
  @OneToMany(() => Resources, (resource) => resource.project, {
    orphanRemoval: true,
  })
  resources = new Collection<Resources>(this);

  @Embedded()
  access: Access[] = [];

  constructor(data: Partial<Projects>) {
    super();
    Object.assign(this, data);
  }
}

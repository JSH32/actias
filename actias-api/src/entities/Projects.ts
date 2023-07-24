import { Collection, Entity, OneToMany, Property } from '@mikro-orm/core';
import { Resources } from './Resources';
import { ActiasBaseEntity } from './BaseEntity';
import { Access } from './Access';
import { BitField } from 'easy-bits';
import { AccessFields } from '../project/acl/accessFields';

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
  @Property({ columnType: 'uuid' })
  ownerId!: string;

  /**
   * All resources owned by the project.
   */
  @OneToMany(() => Resources, (resource) => resource.project, {
    orphanRemoval: true,
  })
  resources = new Collection<Resources>(this);

  /**
   * Default permission bitfield when adding members.
   */
  @Property({
    default: Number(
      new BitField().on(AccessFields.SCRIPT_RESOURCE).serialize(),
    ),
  })
  defaultPermissions!: number;

  @OneToMany(() => Access, (access) => access.project, {
    orphanRemoval: true,
  })
  access = new Collection<Access>(this);

  constructor(data: Partial<Projects>) {
    super();
    Object.assign(this, data);
  }
}

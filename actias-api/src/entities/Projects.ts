import { Collection, Entity, OneToMany, Property } from '@mikro-orm/core';
import { Resources } from './Resources';
import { BaseEntity } from './BaseEntity';

@Entity()
export class Projects extends BaseEntity {
  @Property({ length: 32 })
  name!: string;

  @Property({ columnType: 'uuid' })
  ownerId!: string;

  @OneToMany(() => Resources, (resource) => resource.project)
  resources = new Collection<Resources>(this);

  @Property()
  defaultPermissions!: number;
}

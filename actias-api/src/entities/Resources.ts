import { Cascade, Entity, Enum, ManyToOne, Property } from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Projects } from './Projects';

/**
 * All types of resources that can be owned/accessed.
 */
export enum ResourceType {
  SCRIPT = 'script',
}

/**
 * Individual resources which exist within a project.
 */
@Entity()
export class Resources extends ActiasBaseEntity {
  @Enum(() => ResourceType)
  resourceType!: ResourceType;

  /**
   * ID of this resource in the respective service it originates from.
   */
  @Property()
  serviceId!: string;

  @ManyToOne({ cascade: [Cascade.REMOVE] })
  project!: Projects;

  constructor(resource: Required<Omit<Resources, keyof ActiasBaseEntity>>) {
    super();
    Object.assign(this, resource);
  }
}

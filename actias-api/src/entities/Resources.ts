import { Cascade, Entity, Enum, ManyToOne } from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Projects } from './Projects';

/**
 * All types of resources that can be owned/accessed.
 */
enum ResourceType {
  SCRIPT = 'script',
}

/**
 * Individual resources which exist within a project.
 */
@Entity()
export class Resources extends ActiasBaseEntity {
  @Enum(() => ResourceType)
  resourceType!: ResourceType;

  @ManyToOne({ cascade: [Cascade.REMOVE] })
  project!: Projects;
}

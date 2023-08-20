import {
  Entity,
  EntityManager,
  EntityName,
  Enum,
  EventArgs,
  EventSubscriber,
  ManyToOne,
  Property,
  Subscriber,
} from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Projects } from './Projects';
import { Inject, Injectable, OnModuleInit } from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { script_service } from 'src/protobufs/script_service';

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

  @ManyToOne()
  project!: Projects;

  constructor(resource: Required<Omit<Resources, keyof ActiasBaseEntity>>) {
    super();
    Object.assign(this, resource);
  }
}

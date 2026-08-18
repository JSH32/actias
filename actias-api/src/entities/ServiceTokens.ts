import { Entity, ManyToOne, Property, Unique } from '@mikro-orm/core';
import { Projects } from './Projects';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * A project-scoped machine credential: ACL-scoped like a member, revocable
 * by deletion, and never stored in the clear. The token itself is shown
 * exactly once at creation; only its hash lives here.
 */
@Entity()
export class ServiceTokens extends ActiasBaseEntity {
  /**
   * Human label ("github deploy"), for the token list.
   */
  @Property({ length: 64 })
  name!: string;

  /**
   * sha256 of the full token, hex; the lookup key at authentication.
   */
  @Property({ length: 64 })
  @Unique()
  tokenHash!: string;

  /**
   * First characters of the token, so a listed row can be matched to the
   * secret someone is holding without revealing it.
   */
  @Property({ length: 16 })
  tokenPrefix!: string;

  @ManyToOne({ onDelete: 'cascade' })
  project!: Projects;

  /**
   * Serialized ACL bitfield, the same shape a member's Access row carries.
   */
  @Property()
  permissionBitfield!: string;

  /**
   * Last successful authentication; null until first use.
   */
  @Property({ nullable: true })
  lastUsed?: Date;

  constructor(token: Partial<ServiceTokens>) {
    super();
    Object.assign(this, token);
  }
}

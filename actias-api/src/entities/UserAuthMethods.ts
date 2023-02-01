import {
  Cascade,
  Entity,
  Enum,
  ManyToOne,
  Property,
  Unique,
} from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { Users } from './Users';

export enum AuthMethod {
  PASSWORD = 'password',
  GOOGLE = 'google',
  GITHUB = 'github',
}

/**
 * An authentication method owned by a user.
 * There should be only one auth method of the same type per user.
 */
@Entity()
@Unique({ properties: ['authMethod', 'user'] })
export class UserAuthMethods extends ActiasBaseEntity {
  @ManyToOne({ cascade: [Cascade.REMOVE] })
  user!: Users;

  /**
   * Optional cached username, for OAuth logins.
   */
  @Property()
  cachedUsername?: string;

  @Enum(() => AuthMethod)
  authMethod!: AuthMethod;

  @Property()
  value!: string;

  @Property({ defaultRaw: 'now()' })
  lastAccessed!: Date;

  constructor(data: Partial<UserAuthMethods>) {
    super();
    Object.assign(this, data);
  }
}

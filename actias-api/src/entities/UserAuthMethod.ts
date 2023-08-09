import {
  Property,
  Unique,
  Enum,
  Entity,
  ManyToOne,
  Cascade,
} from '@mikro-orm/core';
import { Users } from './Users';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * An authentication method owned by a user.
 * There should be only one auth method of the same type per user.
 */
@Entity()
@Unique({ properties: ['user', 'method'] })
export class UserAuthMethod extends ActiasBaseEntity {
  /**
   * Optional cached username, for OAuth logins.
   */
  @Property()
  cachedUsername?: string;

  @Enum(() => AuthMethod)
  method!: AuthMethod;

  @Property()
  value!: string;

  @ManyToOne({ cascade: [Cascade.REMOVE] })
  user!: Users;

  constructor(data: Partial<UserAuthMethod>) {
    super();
    Object.assign(this, data);
  }
}

export enum AuthMethod {
  PASSWORD = 'password',
  GOOGLE = 'google',
  GITHUB = 'github',
}

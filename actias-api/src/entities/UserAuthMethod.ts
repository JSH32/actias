import { Embeddable, Property, Unique, Enum } from '@mikro-orm/core';

/**
 * An authentication method owned by a user.
 * There should be only one auth method of the same type per user.
 */
@Embeddable()
export class UserAuthMethod {
  /**
   * Optional cached username, for OAuth logins.
   */
  @Property()
  cachedUsername?: string;

  @Unique()
  @Enum(() => AuthMethod)
  method!: AuthMethod;

  @Property()
  value!: string;

  constructor(data: Partial<UserAuthMethod>) {
    Object.assign(this, data);
  }
}

export enum AuthMethod {
  PASSWORD = 'password',
  GOOGLE = 'google',
  GITHUB = 'github',
}

import { Entity, Enum, ManyToOne, Unique } from '@mikro-orm/core';
import { BaseEntity } from './BaseEntity';
import { Users } from './Users';

export enum AuthMethod {
  PASSWORD = 'password',
  GOOGLE = 'google',
  GITHUB = 'github',
}

@Entity()
@Unique({ properties: ['authMethod', 'user'] })
export class UserAuthMethods extends BaseEntity {
  @ManyToOne()
  user!: Users;

  cachedUsername!: string;

  @Enum(() => AuthMethod)
  authMethod!: AuthMethod;
  value!: string;
  lastAccessed!: Date;
}

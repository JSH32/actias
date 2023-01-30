import { Collection, Entity, OneToMany } from '@mikro-orm/core';
import { BaseEntity } from './BaseEntity';
import { UserAuthMethods } from './UserAuthMethods';

@Entity()
export class Users extends BaseEntity {
  email!: string;
  username!: string;
  admin = false;

  @OneToMany(() => UserAuthMethods, (method) => method.user)
  authMethods = new Collection<UserAuthMethods>(this);
}

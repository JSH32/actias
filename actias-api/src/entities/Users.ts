import {
  Collection,
  Entity,
  Filter,
  OneToMany,
  Property,
} from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';
import { UserAuthMethods } from './UserAuthMethods';

/**
 * Users table, stores simple information about a user account.
 */
@Entity()
export class Users extends ActiasBaseEntity {
  @Property({ length: 320, unique: true })
  email!: string;

  @Property({ length: 36, unique: true })
  username!: string;

  @Property({ default: false })
  admin!: boolean;

  @OneToMany(() => UserAuthMethods, (method) => method.user, {
    orphanRemoval: true,
  })
  authMethods = new Collection<UserAuthMethods>(this);

  constructor(data: Partial<Users>) {
    super();
    Object.assign(this, data);
  }
}

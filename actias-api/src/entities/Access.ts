import {
  Cascade,
  Embeddable,
  ManyToOne,
  Property,
  Unique,
} from '@mikro-orm/core';
import { Users } from './Users';

/**
 * Access control per user owned by a project.
 */
@Embeddable()
@Unique({ properties: ['user'] })
export class Access {
  @ManyToOne({ cascade: [Cascade.REMOVE] })
  user!: Users;

  @Property()
  permissionBitfield!: number;

  constructor(access: Required<Access>) {
    Object.assign(this, access);
  }
}

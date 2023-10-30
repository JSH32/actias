import { Entity, Property } from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * Registration code table, stores a code that can be used to register a new user.
 */
@Entity()
export class RegistrationCodes extends ActiasBaseEntity {
  @Property({ defaultRaw: '1' })
  uses: number;

  constructor(data: Partial<RegistrationCodes>) {
    super();
    Object.assign(this, data);
  }
}

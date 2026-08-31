import { Entity, Property, Unique } from '@mikro-orm/core';
import { ActiasBaseEntity } from './BaseEntity';

/**
 * Somebody who asked to hear when the project moves. There is no mail
 * out of here yet: this is the list a future announcement would be sent
 * to, and until then it is the only signal about who is waiting.
 */
@Entity()
export class InterestSignup extends ActiasBaseEntity {
  /**
   * Lowercased before it is stored, so the same address cannot arrive
   * twice wearing different capitals.
   */
  @Property()
  @Unique()
  email: string;

  /**
   * Which surface the address came from, so a second one later can be
   * told apart from the landing page without guessing by date.
   */
  @Property()
  source: string;

  constructor(data: Partial<InterestSignup>) {
    super();
    Object.assign(this, data);
  }
}

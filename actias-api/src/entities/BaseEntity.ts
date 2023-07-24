import { Entity, PrimaryKey, Property } from '@mikro-orm/core';

@Entity({ abstract: true })
export abstract class ActiasBaseEntity {
  @PrimaryKey({ type: 'uuid', defaultRaw: 'gen_random_uuid()' })
  id: string;

  @Property({ defaultRaw: 'now()' })
  createdAt: Date;

  @Property({ onUpdate: () => new Date(), defaultRaw: 'now()' })
  updatedAt: Date;
}

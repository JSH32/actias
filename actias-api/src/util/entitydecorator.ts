import { EntityManager, EntityName } from '@mikro-orm/core';
import { Injectable, Param, PipeTransform, Scope } from '@nestjs/common';

/**
 * Pipe to convert a primary key to the respective entity.
 * @param type of the entity
 * @returns entity.
 */
export const EntityPipe = <T extends object>(type: EntityName<T>) => {
  @Injectable({ scope: Scope.REQUEST })
  class EntityPipe<T extends object> implements PipeTransform {
    constructor(readonly em: EntityManager) {}

    async transform(value: any): Promise<T> {
      const entity = await this.em.findOneOrFail(type, value);
      return entity as unknown as T;
    }
  }

  return EntityPipe;
};

/**
 * Shorthand for {@link EntityPipe} that assumes its a {@link Param}.
 * @param paramName name of parameter in url string
 * @param entity type of entity
 * @returns entity
 */
export const EntityParam = <T extends object>(
  paramName: string,
  entity: EntityName<T>,
) => Param(paramName, EntityPipe(entity));

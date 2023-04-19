import { EntityName, EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable, Param, PipeTransform, Scope } from '@nestjs/common';

/**
 * Pipe to convert a primary key to the respective entity.
 * @param type of the entity
 * @returns entity.
 */
export const EntityPipe = <T extends object>(type: EntityName<T>) => {
  @Injectable({ scope: Scope.REQUEST })
  class EntityPipe<T extends object> implements PipeTransform {
    constructor(
      @InjectRepository(type)
      readonly manager: EntityRepository<T>,
    ) {}

    async transform(value: any): Promise<T> {
      return await this.manager.findOneOrFail(value);
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

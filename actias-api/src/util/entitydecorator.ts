import { EntityManager, EntityName, FilterQuery } from '@mikro-orm/core';
import {
  BadRequestException,
  Injectable,
  Param,
  PipeTransform,
} from '@nestjs/common';

const UUID_SHAPE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * The id every routed entity keys by (BaseEntity's uuid), refused with
 * a 400 when the url carries anything else; the database never sees a
 * malformed id, so it can never turn one into a server error.
 */
export const requireUuid = (value: unknown): string => {
  if (typeof value !== 'string' || !UUID_SHAPE.test(value)) {
    throw new BadRequestException(
      `'${String(value)}' is not a valid identifier.`,
    );
  }
  return value;
};

/**
 * Pipe to convert a primary key to the respective entity.
 * @param type of the entity
 * @returns entity.
 */
export const EntityPipe = <T extends object>(type: EntityName<T>) => {
  @Injectable()
  class EntityPipe implements PipeTransform {
    constructor(readonly em: EntityManager) {}

    async transform(value: unknown): Promise<T> {
      return await this.em.findOneOrFail(
        type,
        requireUuid(value) as unknown as FilterQuery<T>,
      );
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

import { applyDecorators, Type } from '@nestjs/common';
import {
  ApiExtraModels,
  ApiOkResponse,
  ApiProperty,
  getSchemaPath,
} from '@nestjs/swagger';

export class PaginatedDto<T> {
  items: T[];

  @ApiProperty({ description: 'Current page.' })
  page: number;

  @ApiProperty({ description: 'How many pages exist.' })
  totalPages: number;

  constructor(data: Partial<PaginatedDto<T>>) {
    Object.assign(this, data);
  }
}

/**
 * Paginated OpenAPI decorator, apply this to everywhere with a paginated response type.
 * @param dataDto type of the elements.
 */
export const ApiOkResponsePaginated = <DataDto extends Type<unknown>>(
  dataDto: DataDto,
) =>
  applyDecorators(
    ApiExtraModels(PaginatedDto, dataDto),
    ApiOkResponse({
      schema: {
        allOf: [
          { $ref: getSchemaPath(PaginatedDto) },
          {
            properties: {
              data: {
                type: 'array',
                items: { $ref: getSchemaPath(dataDto) },
              },
            },
          },
        ],
      },
    }),
  );

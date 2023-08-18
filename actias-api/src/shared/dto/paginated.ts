import { applyDecorators, Type } from '@nestjs/common';
import {
  ApiExtraModels,
  ApiOkResponse,
  ApiProperty,
  getSchemaPath,
} from '@nestjs/swagger';

export class PaginatedResponseDto<T> {
  items: T[];

  @ApiProperty({ description: 'Current page.' })
  page: number;

  @ApiProperty({ description: 'Last possible page.' })
  lastPage: number;

  constructor(page: Required<PaginatedResponseDto<T>>) {
    Object.assign(this, page);
  }

  static fromArray<T>(
    page: number,
    lastPage: number,
    data: T[],
  ): PaginatedResponseDto<T> {
    return new this({
      items: data,
      page,
      lastPage,
    });
  }
}

export const ApiOkResponsePaginated = <DataDto extends Type<unknown>>(
  dataDto: DataDto,
) =>
  applyDecorators(
    ApiExtraModels(PaginatedResponseDto, dataDto),
    ApiOkResponse({
      schema: {
        allOf: [
          { $ref: getSchemaPath(PaginatedResponseDto) },
          {
            properties: {
              items: {
                type: 'array',
                items: { $ref: getSchemaPath(dataDto) },
              },
            },
          },
        ],
      },
    }),
  );

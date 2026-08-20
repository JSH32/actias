import { ApiProperty } from '@nestjs/swagger';
import { IsArray, IsOptional, IsString } from 'class-validator';

/**
 * One queue or database a project holds: declared by a live contract,
 * present in the instance directory, or both.
 */
export class ResourceInstanceDto {
  @ApiProperty()
  name: string;

  @ApiProperty()
  scriptId: string;

  @ApiProperty({ description: 'Public identifier of the owning script.' })
  scriptIdentifier: string;

  @ApiProperty({
    description:
      'Data exists but no live revision declares it; the platform keeps it until it is deleted explicitly.',
  })
  orphaned: boolean;
}

export class QueueStatsDto {
  @ApiProperty()
  depth: number;

  @ApiProperty({ required: false, nullable: true })
  oldestPending?: number;

  @ApiProperty()
  deadLetters: number;
}

export class TableInfoDto {
  @ApiProperty()
  name: string;

  @ApiProperty()
  rows: number;
}

export class SqlQueryDto {
  @ApiProperty()
  @IsString()
  sql: string;

  @ApiProperty({ required: false, type: 'array', items: {} })
  @IsOptional()
  @IsArray()
  params?: unknown[];
}

export class SqlRowsDto {
  @ApiProperty({ type: 'array', items: {} })
  rows: unknown[];
}

export class QueueEventDto {
  @ApiProperty()
  seq: number;

  @ApiProperty()
  at: number;

  @ApiProperty()
  kind: string;

  @ApiProperty()
  detail: string;
}

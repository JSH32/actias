import { ApiProperty } from '@nestjs/swagger';
import { IsArray, IsOptional, IsString } from 'class-validator';

/**
 * One queue or database a project holds: declared by a live contract,
 * present in the instance directory, or both. Identity is the name,
 * scoped to the project; the declaring script is metadata.
 */
export class ResourceInstanceDto {
  @ApiProperty()
  name: string;

  @ApiProperty({
    description:
      'Public identifier of a script declaring it; empty when only the directory remembers it.',
  })
  declaredBy: string;

  @ApiProperty({
    description:
      'Data exists but no live revision declares it; the platform keeps it until it is deleted explicitly.',
  })
  orphaned: boolean;
}

export class QueueStatsDto {
  @ApiProperty({ description: 'Every message still queued.' })
  depth: number;

  @ApiProperty({ description: 'Messages due now, in delivery.' })
  inFlight: number;

  @ApiProperty({ required: false, nullable: true })
  oldestPending?: number;

  @ApiProperty()
  deadLetters: number;
}

/** One live or dead message row, as the inspector's table shows it. */
export class QueueMessageDto {
  @ApiProperty()
  id: number;

  @ApiProperty({ description: 'pending, in-flight or dead.' })
  state: string;

  @ApiProperty()
  attempts: number;

  @ApiProperty({ description: 'Payload prefix.' })
  preview: string;

  @ApiProperty({ description: 'Payload size in bytes.' })
  size: number;

  @ApiProperty()
  enqueuedMs: number;

  @ApiProperty({ required: false, nullable: true })
  nextMs?: number;

  @ApiProperty({ required: false, nullable: true })
  diedMs?: number;
}

export class ColumnInfoDto {
  @ApiProperty()
  name: string;

  @ApiProperty({ description: 'Declared SQLite type; empty when untyped.' })
  type: string;

  @ApiProperty()
  notNull: boolean;

  @ApiProperty()
  primaryKey: boolean;
}

export class TableInfoDto {
  @ApiProperty()
  name: string;

  @ApiProperty()
  rows: number;

  @ApiProperty({ type: [ColumnInfoDto] })
  columns: ColumnInfoDto[];
}

/** The database viewer's one-call read: file size plus table shapes. */
export class DatabaseOverviewDto {
  @ApiProperty({ description: 'Database file size in bytes.' })
  sizeBytes: number;

  @ApiProperty({ type: [TableInfoDto] })
  tables: TableInfoDto[];
}

/** One durable object instance the directory knows. */
export class ObjectInstanceDto {
  @ApiProperty({ description: 'The object class.' })
  class: string;

  @ApiProperty({ description: 'The instance name.' })
  name: string;

  @ApiProperty({
    description: 'Public identifier of the script whose code it runs.',
  })
  declaredBy: string;
}

/** One page of a class's instances, with how many match in all. */
export class ObjectPageDto {
  @ApiProperty({ type: [ObjectInstanceDto] })
  items: ObjectInstanceDto[];

  @ApiProperty({
    description: 'Instances matching the filter across every page.',
  })
  total: number;
}

/** How many instances one class holds; the rail shows classes, not names. */
export class ClassCountDto {
  @ApiProperty()
  class: string;

  @ApiProperty()
  count: number;
}

export class RetriedDto {
  @ApiProperty({ description: 'How many dead letters were requeued.' })
  requeued: number;
}

/** One edge in a publisher's follower table. */
export class FollowerEdgeDto {
  @ApiProperty({ description: "'object' (durable) or 'connection'." })
  kind: string;

  @ApiProperty({ description: "The follower's identity, 'Class/name'." })
  follower: string;

  @ApiProperty({
    required: false,
    nullable: true,
    description: 'Connection edges only: the endpoint connection id.',
  })
  connection: string | null;

  @ApiProperty({ description: 'The topic this edge listens on.' })
  topic: string;

  @ApiProperty({
    required: false,
    nullable: true,
    type: 'object',
    additionalProperties: true,
    description: 'Equality filter on event data fields, when set.',
  })
  filter: Record<string, unknown> | null;

  @ApiProperty({ description: 'Last event sequence this edge passed.' })
  cursor: number;

  @ApiProperty({
    required: false,
    nullable: true,
    description: 'Undelivered events behind the log head; durable edges only.',
  })
  lag: number | null;

  @ApiProperty({ description: 'Consecutive failed deliveries so far.' })
  attempts: number;

  @ApiProperty({
    description: 'When delivery retries next (unix ms); 0 when not due.',
  })
  nextAt: number;
}

/** The publisher's edge table plus its event-log head. */
export class FollowersDto {
  @ApiProperty({ description: "Newest event sequence in the publisher's log." })
  head: number;

  @ApiProperty({ type: [FollowerEdgeDto] })
  edges: FollowerEdgeDto[];
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

  @ApiProperty({
    description:
      'Structured event detail: message id, payload preview, producer, per-attempt error.',
    type: 'object',
    additionalProperties: true,
  })
  detail: Record<string, unknown>;
}

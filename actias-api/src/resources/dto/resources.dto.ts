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

  @ApiProperty({ description: 'Unix ms of the first claim.' })
  createdMs: number;

  @ApiProperty({
    description: 'When the platform deletes it if untouched; 0 = never.',
  })
  expireAtMs: number;

  @ApiProperty({
    description:
      'Tombstone time; nonzero is a deletion in progress the janitor is finishing.',
  })
  deletedAtMs: number;

  @ApiProperty({ description: "The pending alarm's due time; 0 = none." })
  alarmDueMs: number;

  @ApiProperty({
    description: 'The lease holder; empty = cold, next touch revives.',
  })
  nodeId: string;
}

/** What a deletion request settled. */
export class DeleteOutcomeDto {
  @ApiProperty({
    description:
      'Rows tombstoned by this request; the janitor finishes each one.',
  })
  deleting: number;
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
/** One field a class's directory declares, as its publish recorded it. */
export class DirectoryFieldDto {
  @ApiProperty()
  name: string;

  @ApiProperty({
    description:
      'string, integer, number, boolean or array. The store binds each value as the kind declared, and a filter is checked against it.',
  })
  kind: string;
}

export class ClassCountDto {
  @ApiProperty()
  class: string;

  @ApiProperty()
  count: number;

  @ApiProperty({
    description:
      "Whether the class's current revision declares a directory, so the console offers a search only where one exists.",
  })
  hasDirectory: boolean;

  @ApiProperty({
    type: [DirectoryFieldDto],
    description:
      'The fields that directory declares, so a filter can be typed against the same set the worker enforces. Empty for a class with no directory, and for one published before fields were declared.',
  })
  directoryFields: DirectoryFieldDto[];

  @ApiProperty({
    type: [String],
    description:
      "The methods the class's current revision declares, by name, so a shell can offer what an instance answers to. Which of them write is not declarable; a read-only session refuses at the call.",
  })
  methods: string[];
}

export class ObjectCallDto {
  @ApiProperty({ description: 'The method to call on the instance.' })
  @IsString()
  method: string;

  @ApiProperty({
    required: false,
    type: 'array',
    items: {},
    description: 'Positional arguments, as json values.',
  })
  @IsOptional()
  @IsArray()
  args?: unknown[];
}

export class ObjectCallResultDto {
  @ApiProperty({
    description:
      'What the method returned, as json text; "null" for nothing. Text rather than a typed value because a method returns whatever it returns, and every generated client reads json text the same way (the same convention as condition values and entry fields).',
  })
  valueJson: string;
}

export class ShellRunDto {
  @ApiProperty({
    description:
      'The chunk to run: any Luau, as typed or pasted. Its return value is the result.',
  })
  @IsString()
  source: string;

  @ApiProperty({
    required: false,
    description: 'Wall budget in seconds; the node caps it.',
  })
  @IsOptional()
  wallSecs?: number;

  @ApiProperty({
    required: false,
    description:
      'Whether the session is in write mode. Off, the chunk still runs, and the vm refuses kv set/delete, database exec and method calls inside it.',
  })
  @IsOptional()
  write?: boolean;
}

export class ShellOutcomeDto {
  @ApiProperty({
    description: 'The chunk\'s return value as json text; "null" for nothing.',
  })
  valueJson: string;

  @ApiProperty({
    type: [String],
    description: 'Every print and log line, in order.',
  })
  output: string[];

  @ApiProperty({
    required: false,
    description: "The chunk's own error, when it failed.",
  })
  error?: string;

  @ApiProperty({ description: 'Work units the run consumed.' })
  work: number;

  @ApiProperty({ description: 'Milliseconds the run took on the node.' })
  wallMs: number;
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

/** One live connection on the cluster, either direction: what the
 * console lists. Runtime state, never contract. */
export class ConnectionDto {
  @ApiProperty({ description: 'Node-local connection id.' })
  id: string;

  @ApiProperty({ description: 'The declared connection class running the wire.' })
  connectionClass: string;

  @ApiProperty({ description: "The identity it speaks as, 'Class/name'." })
  identity: string;

  @ApiProperty({ description: "'inbound' (a client's upgrade) or 'outbound' (dialled by the project)." })
  direction: string;

  @ApiProperty({
    required: false,
    nullable: true,
    description: "The far side's host, outbound only.",
  })
  peer: string | null;

  @ApiProperty({ description: 'The node holding the wire.' })
  node: string;

  @ApiProperty({ description: 'The script whose revision opened it.' })
  scriptId: string;

  @ApiProperty({ description: 'Unix milliseconds the wire opened.' })
  openedAt: number;

  @ApiProperty({ description: "'new', 'warm' (holding a vm) or 'hibernated' (wire kept, vm dropped)." })
  status: string;

  @ApiProperty({ description: 'Edges the connection holds right now.' })
  follows: number;
}

/** One typed pair in an object's key-value state, the kv service's
 * encoding: `type` records how `value` parses. */
export class StatePairDto {
  @ApiProperty()
  key: string;

  @ApiProperty({ description: "How 'value' parses: the kv typed-pair kind." })
  type: string;

  @ApiProperty({ description: 'The stored value, as encoded text.' })
  value: string;
}

/** An object's reserved state pairs, in key order. */
export class StateDto {
  @ApiProperty({ type: [StatePairDto] })
  entries: StatePairDto[];
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

/** One filter on one directory field. A bare value is equality; the
 * operator names match the Lua surface exactly, so what an author
 * types and what the console sends are one vocabulary. */
export class DirectoryConditionDto {
  @ApiProperty({
    description:
      'Field name as the class\'s directory function returned it; nested tables arrive flattened, so "location.region" is one field.',
  })
  field: string;

  @ApiProperty({
    description:
      'eq, ne, lt, lte, gt, gte, one_of, starts_with, contains or exists. one_of rather than in, because in is a Lua keyword.',
  })
  op: string;

  @ApiProperty({
    description:
      'The operand, json-encoded: a scalar, an array for one_of, or a boolean for exists.',
  })
  valueJson: string;
}

/** A conjunction of conditions, with nested combinators. Sent as a
 * tree rather than a query string: the worker turns it into
 * parameterized sql, so nothing a caller sends can shape an
 * identifier. */
export class DirectoryWhereDto {
  @ApiProperty({ type: [DirectoryConditionDto], required: false })
  conditions?: DirectoryConditionDto[];

  @ApiProperty({
    type: [DirectoryWhereDto],
    required: false,
    description: 'OR over sub-wheres.',
  })
  any?: DirectoryWhereDto[];

  @ApiProperty({
    type: [DirectoryWhereDto],
    required: false,
    description: 'Explicit AND, for grouping inside an any.',
  })
  all?: DirectoryWhereDto[];

  @ApiProperty({
    type: [DirectoryWhereDto],
    required: false,
    description: 'NOT over sub-wheres.',
  })
  none?: DirectoryWhereDto[];
}

export class DirectoryOrderDto {
  @ApiProperty()
  field: string;

  @ApiProperty({ required: false })
  descending?: boolean;
}

/** What a directory listing asks for. */
export class DirectoryQueryDto {
  @ApiProperty({ type: DirectoryWhereDto, required: false })
  where?: DirectoryWhereDto;

  @ApiProperty({ type: [DirectoryOrderDto], required: false })
  order?: DirectoryOrderDto[];

  @ApiProperty({
    required: false,
    description: 'Rows this page may carry; the worker caps it.',
  })
  limit?: number;

  @ApiProperty({ required: false, description: 'From a previous page.' })
  cursor?: string;
}

/** One object's row. */
export class DirectoryEntryDto {
  @ApiProperty({
    description: 'The instance name; what you call to reach the object.',
  })
  name: string;

  @ApiProperty()
  objectId: string;

  @ApiProperty({
    type: 'object',
    additionalProperties: { type: 'string' },
    description:
      'Field name to json-encoded value, for the fields this object has. A field the object lacks is simply absent.',
  })
  fields: Record<string, string>;
}

/** One row of a verified listing. */
export class VisitEntryDto {
  @ApiProperty({ type: DirectoryEntryDto })
  entry: DirectoryEntryDto;

  @ApiProperty({
    description:
      'The row could not be checked against its object and is served anyway, saying so: dropping it would invent the false negative the directory refuses.',
  })
  unverified: boolean;

  @ApiProperty({ required: false, description: 'Why, when unverified.' })
  reason?: string;
}

/** One page of a verified listing.
 *
 * Every candidate was checked against its object\'s own settled state:
 * rows that stopped matching are gone, fresher rows are served fresh,
 * and the unverifiable are flagged. The limit bounds candidates
 * examined, so a short page with a cursor is normal, not the end. */
export class VisitPageDto {
  @ApiProperty({ type: [VisitEntryDto] })
  entries: VisitEntryDto[];

  @ApiProperty({
    required: false,
    description: 'Feeds the next page; absent on the last.',
  })
  cursor?: string;

  @ApiProperty({
    type: [String],
    description: 'Fields still building; a query naming one is refused.',
  })
  building: string[];
}

/** One page of a directory listing.
 *
 * The rows are a snapshot as of each object\'s last saved write, so a
 * listing chooses which objects to call; the object itself is the
 * truth. */
export class DirectoryPageDto {
  @ApiProperty({ type: [DirectoryEntryDto] })
  entries: DirectoryEntryDto[];

  @ApiProperty({
    required: false,
    description: 'Feeds the next page; absent on the last.',
  })
  cursor?: string;

  @ApiProperty({
    type: [String],
    description:
      'Fields the class has seen but not finished backfilling. A query naming one is refused; this reports the rest so progress is visible rather than a column mysteriously missing.',
  })
  building: string[];
}

/** What an operator's rebuild of one class recovered.
 *
 * Every row comes from an object's shipping manifest, so a rebuild is
 * a metadata copy: nothing is woken, and no object's file is opened. */
export class DirectoryRebuiltDto {
  @ApiProperty({
    description: 'Identities the placement store still lists as live.',
  })
  live: number;

  @ApiProperty({ description: 'Rows recovered from manifests.' })
  rows: number;

  @ApiProperty({
    description:
      'Live objects whose manifest carries no row yet. Not an error: nothing has settled to copy, and a backfill is what covers them.',
  })
  withoutRow: number;

  @ApiProperty({
    description: 'Rows retired because the object no longer exists.',
  })
  tombstones: number;

  @ApiProperty({
    description:
      'Whether a node did the work. False means another node holds the class and is rebuilding it already.',
  })
  held: boolean;
}

import { ApiProperty } from '@nestjs/swagger';

export class NamespaceDto {
  projectId: string;
  /**
   * Namespace identifier (project scoped)
   */
  name: string;
  /**
   * Amount of pairs in the namespace.
   */
  count: number;
}

/** The value types this api speaks, by name. The kv service carries the
 * same set as a proto enum, whose ordinals are the order below. */
export enum PairType {
  STRING = 'STRING',
  NUMBER = 'NUMBER',
  INTEGER = 'INTEGER',
  BOOLEAN = 'BOOLEAN',
  JSON = 'JSON',
}

/** [`PairType`] in the proto enum's declared order, which is how a value
 * arrives from the kv service. */
const BY_ORDINAL: PairType[] = [
  PairType.STRING,
  PairType.NUMBER,
  PairType.INTEGER,
  PairType.BOOLEAN,
  PairType.JSON,
];

/**
 * The name for a type as the kv service sends it. Proto3 elides the zero
 * value, so an absent type is a string, and a client library may hand
 * over either the ordinal or the proto's own spelling.
 */
export function pairTypeOf(value: unknown): PairType {
  if (typeof value === 'number') return BY_ORDINAL[value] ?? PairType.STRING;
  if (typeof value === 'string') {
    const name = value.replace(/^VALUE_TYPE_/, '').toUpperCase();
    return (PairType as Record<string, PairType>)[name] ?? PairType.STRING;
  }
  return PairType.STRING;
}

export class PairDto {
  projectId: string;
  namespace: string;
  /** The stored value is always text; this names how to read it back. */
  @ApiProperty({
    enum: PairType,
    enumName: 'PairType',
  })
  type: PairType;
  /**
   * Seconds until the pair expires; absent for one that never does.
   * Declared optional so a generated client does not demand a field
   * the service omits.
   */
  @ApiProperty({ required: false })
  ttl?: number;
  key: string;
  value: string;

  constructor(pair: Partial<PairDto>) {
    Object.assign(this, pair);
  }
}

export class ListNamespaceDto {
  pageSize: number;
  /**
   * Token used to fetch next page.
   * Not provided on last page.
   */
  token?: string;
  pairs: PairDto[];
}

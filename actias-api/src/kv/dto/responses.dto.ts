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

export enum PairType {
  STRING = 0,
  NUMBER = 1,
  INTEGER = 2,
  BOOLEAN = 3,
  JSON = 4,
}

export class PairDto {
  // Project that this key belongs to.
  projectId: string;
  // Grouped namespace that this belongs to.
  namespace: string;
  // The value is always stored as a string, this metadata helps for parsing.
  @ApiProperty({
    enum: PairType,
  })
  type: string;
  // TTL (time to live)
  ttl: number;
  // Unique key.
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

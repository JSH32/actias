import { ApiProperty } from '@nestjs/swagger';
import { ServiceTokens } from 'src/entities/ServiceTokens';
import { ACCESS_KEYS, getListFromBitfield } from '../acl/accessFields';

/**
 * Creates a project service token.
 */
export class CreateServiceTokenDto {
  /**
   * Human label for the token list, e.g. "github deploy".
   */
  name: string;

  /**
   * Access field names granted to the token (see the acl list shape).
   * Omitted, the token can manage scripts and kv but not permissions.
   */
  @ApiProperty({ enum: ACCESS_KEYS, isArray: true, required: false })
  access?: string[];
}

/**
 * One service token, as listed. The secret itself is never here.
 */
export class ServiceTokenDto {
  id: string;
  name: string;

  /**
   * First characters of the secret, to match a held token to its row.
   */
  tokenPrefix: string;

  /**
   * Access field names to whether the token holds them.
   */
  access: Record<string, boolean>;

  createdAt: Date;
  lastUsed?: Date;

  constructor(token: ServiceTokens) {
    this.id = token.id;
    this.name = token.name;
    this.tokenPrefix = token.tokenPrefix;
    this.access = getListFromBitfield(token.permissionBitfield);
    this.createdAt = token.createdAt;
    this.lastUsed = token.lastUsed;
  }
}

/**
 * The creation response: the only time the secret is ever shown.
 */
export class CreatedServiceTokenDto extends ServiceTokenDto {
  /**
   * The full token. Store it now; it cannot be retrieved again.
   */
  token: string;

  constructor(entity: ServiceTokens, token: string) {
    super(entity);
    this.token = token;
  }
}

import {
  BadRequestException,
  Body,
  Controller,
  Delete,
  Get,
  Param,
  Post,
  UseGuards,
} from '@nestjs/common';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { EntityManager } from '@mikro-orm/postgresql';
import { BitField } from 'easy-bits';
import { createHash, randomBytes } from 'crypto';

import { AuthGuard } from 'src/auth/auth.guard';
import { SERVICE_TOKEN_PREFIX } from 'src/auth/auth.service';
import { Projects } from 'src/entities/Projects';
import { ServiceTokens } from 'src/entities/ServiceTokens';
import { AclByProject, AclGuard } from './acl/acl.guard';
import { AccessFields } from './acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { MessageResponseDto } from 'src/shared/dto/message';
import {
  CreateServiceTokenDto,
  CreatedServiceTokenDto,
  ServiceTokenDto,
} from './dto/tokens.dto';

/**
 * Project-scoped machine credentials: ACL-scoped like a member, revocable
 * by deletion, secret shown exactly once at creation.
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('tokens')
@ApiBearerAuth()
@Controller('project/:project/tokens')
export class ServiceTokensController {
  constructor(private readonly em: EntityManager) {}

  /**
   * Create a service token. The response is the only time the secret is
   * shown; only its hash is stored.
   */
  @Post()
  @AclByProject(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async createToken(
    @EntityParam('project', Projects) project: Projects,
    @Body() request: CreateServiceTokenDto,
  ): Promise<CreatedServiceTokenDto> {
    const bits = new BitField<AccessFields>();
    if (request.access?.length) {
      for (const name of request.access) {
        const field = AccessFields[name as keyof typeof AccessFields];
        if (typeof field !== 'number') {
          throw new BadRequestException(`Unknown access field '${name}'.`);
        }
        bits.on(field);
      }
    } else {
      // The automation default: deploy scripts and manage kv, but never
      // touch membership or mint further credentials.
      bits.on(AccessFields.SCRIPT_RESOURCE);
      bits.on(AccessFields.KV_RESOURCE);
    }

    const token = `${SERVICE_TOKEN_PREFIX}${randomBytes(24).toString('hex')}`;

    const entity = new ServiceTokens({
      name: request.name,
      tokenHash: createHash('sha256').update(token).digest('hex'),
      tokenPrefix: token.slice(0, 15),
      project,
      permissionBitfield: bits.serialize().toString(),
    });
    await this.em.persistAndFlush(entity);

    return new CreatedServiceTokenDto(entity, token);
  }

  /**
   * List the project's service tokens. Secrets are never listed.
   */
  @Get()
  @AclByProject(AccessFields.PERMISSIONS_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listTokens(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ServiceTokenDto[]> {
    const tokens = await this.em.find(ServiceTokens, { project });
    return tokens.map((token) => new ServiceTokenDto(token));
  }

  /**
   * Revoke a token. Revocation is deletion: the hash is gone, so the held
   * secret can never authenticate again.
   */
  @Delete(':token')
  @AclByProject(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async revokeToken(
    @EntityParam('project', Projects) project: Projects,
    @Param('token') tokenId: string,
  ): Promise<MessageResponseDto> {
    const token = await this.em.findOneOrFail(ServiceTokens, {
      id: tokenId,
      project,
    });
    await this.em.removeAndFlush(token);

    return new MessageResponseDto('Service token revoked.');
  }
}

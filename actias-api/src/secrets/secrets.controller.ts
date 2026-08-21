import {
  Body,
  Controller,
  Delete,
  Get,
  Inject,
  Param,
  Put,
  UseGuards,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';

import { AuthGuard } from 'src/auth/auth.guard';
import { User } from 'src/auth/user.decorator';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { AccessFields } from 'src/project/acl/accessFields';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { secret_service } from 'src/protobufs/secret_service';
import { MessageResponseDto } from 'src/shared/dto/message';
import { EntityParam } from 'src/util/entitydecorator';
import { SecretDto, SetSecretDto } from './dto/secrets.dto';

/**
 * Project secrets, forwarded to the secret service: versioned and
 * envelope-encrypted there, write-only here. Values leave the platform
 * only through worker resolution when a script declares them.
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('secrets')
@ApiBearerAuth()
@Controller('project/:project/secrets')
export class SecretsController {
  private secretService: secret_service.SecretService;

  constructor(
    @Inject('SECRET_SERVICE') private readonly client: ClientGrpc,
  ) {}

  onModuleInit() {
    this.secretService =
      this.client.getService<secret_service.SecretService>('SecretService');
  }

  /**
   * List a project's live secrets: names and metadata, never values.
   */
  @Get()
  @AclByProject(AccessFields.SECRETS_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listSecrets(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<SecretDto[]> {
    const response = await lastValueFrom(
      this.secretService
        .listSecrets({ projectId: project.id })
        .pipe(toHttpException()),
    );

    return (response.secrets || []).map((meta) => new SecretDto(meta));
  }

  /**
   * Set or rotate a secret. Every write is a new immutable version; there
   * is no way to read a value back.
   */
  @Put(':name')
  @AclByProject(AccessFields.SECRETS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async putSecret(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @User() user: Users,
    @Body() request: SetSecretDto,
  ): Promise<SecretDto> {
    const meta = await lastValueFrom(
      this.secretService
        .setSecret({
          projectId: project.id,
          name,
          value: request.value,
          createdBy: user.id,
        })
        .pipe(toHttpException()),
    );

    return new SecretDto(meta);
  }

  /**
   * Delete a secret. The name disappears and scripts stop resolving it;
   * workflow runs that pinned a version keep the credentials they
   * started with.
   */
  @Delete(':name')
  @AclByProject(AccessFields.SECRETS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async deleteSecret(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<MessageResponseDto> {
    await lastValueFrom(
      this.secretService
        .deleteSecret({ projectId: project.id, name })
        .pipe(toHttpException()),
    );

    return { message: `Secret '${name}' deleted.` };
  }
}

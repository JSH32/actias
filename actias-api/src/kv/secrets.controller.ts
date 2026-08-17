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
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { AccessFields } from 'src/project/acl/accessFields';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { kv_service } from 'src/protobufs/kv_service';
import { EntityParam } from 'src/util/entitydecorator';
import { AuthGuard } from 'src/auth/auth.guard';
import { MessageResponseDto } from 'src/shared/dto/message';
import { SecretsService } from './secrets.service';
import { SECRETS_NAMESPACE } from './reserved';
import { SetSecretDto } from './dto/requests.dto';

/**
 * Project secrets: written and listed here, decrypted only by the worker
 * when a script declares them. Values never come back out of this api.
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('secrets')
@ApiBearerAuth()
@Controller('project/:project/secrets')
export class SecretsController {
  private kvService: kv_service.KvService;

  constructor(
    @Inject('KV_SERVICE') private readonly client: ClientGrpc,
    private readonly secrets: SecretsService,
  ) {}

  onModuleInit() {
    this.kvService = this.client.getService<kv_service.KvService>('KvService');
  }

  /**
   * List the names of a project's secrets. Values are never returned.
   */
  @Get()
  @AclByProject(AccessFields.PERMISSIONS_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listSecrets(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<string[]> {
    const pairs = await lastValueFrom(
      this.kvService
        .listPairs({
          projectId: project.id,
          namespace: SECRETS_NAMESPACE,
          pageSize: 100,
        })
        .pipe(toHttpException()),
    );

    return (pairs.pairs || []).map((pair) => pair.key);
  }

  /**
   * Set a secret, encrypting it at rest. Overwrites an existing value.
   */
  @Put(':name')
  @AclByProject(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async putSecret(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
    @Body() request: SetSecretDto,
  ): Promise<MessageResponseDto> {
    await lastValueFrom(
      this.kvService
        .setPairs({
          pairs: [
            {
              projectId: project.id,
              namespace: SECRETS_NAMESPACE,
              key: name,
              value: this.secrets.encrypt(request.value),
              type: kv_service.ValueType.VALUE_TYPE_STRING,
            },
          ],
        })
        .pipe(toHttpException()),
    );

    return { message: `Secret '${name}' set.` };
  }

  /**
   * Delete a secret.
   */
  @Delete(':name')
  @AclByProject(AccessFields.PERMISSIONS_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async deleteSecret(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<MessageResponseDto> {
    await lastValueFrom(
      this.kvService
        .deletePairs({
          pairs: [
            {
              projectId: project.id,
              namespace: SECRETS_NAMESPACE,
              key: name,
            },
          ],
        })
        .pipe(toHttpException()),
    );

    return { message: `Secret '${name}' deleted.` };
  }
}

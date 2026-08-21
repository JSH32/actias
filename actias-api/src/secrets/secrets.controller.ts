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
import { EntityManager } from '@mikro-orm/postgresql';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';

import { AuthGuard } from 'src/auth/auth.guard';
import { User } from 'src/auth/user.decorator';
import { Projects } from 'src/entities/Projects';
import { Users } from 'src/entities/Users';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { AccessFields } from 'src/project/acl/accessFields';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { script_service } from 'src/protobufs/script_service';
import { secret_service } from 'src/protobufs/secret_service';
import { MessageResponseDto } from 'src/shared/dto/message';
import { EntityParam } from 'src/util/entitydecorator';
import { SecretDto, SecretVersionDto, SetSecretDto } from './dto/secrets.dto';

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
  private scriptService: script_service.ScriptService;

  constructor(
    @Inject('SECRET_SERVICE') private readonly client: ClientGrpc,
    @Inject('SCRIPT_SERVICE') private readonly scriptClient: ClientGrpc,
    private readonly em: EntityManager,
  ) {}

  onModuleInit() {
    this.secretService =
      this.client.getService<secret_service.SecretService>('SecretService');
    this.scriptService =
      this.scriptClient.getService<script_service.ScriptService>(
        'ScriptService',
      );
  }

  /** Usernames for a set of author ids; deleted accounts read empty. */
  private async usernames(ids: string[]): Promise<Map<string, string>> {
    const distinct = [...new Set(ids.filter(Boolean))];
    if (distinct.length === 0) return new Map();
    const users = await this.em.find(Users, { id: { $in: distinct } });
    return new Map(users.map((user) => [user.id, user.username]));
  }

  /** Which live script declares each secret name, from the same contract
   * capabilities the script detail renders. */
  private async declarers(
    projectId: string,
  ): Promise<Map<string, { script: string; revision: string }>> {
    const page = await lastValueFrom(
      this.scriptService
        .listScripts({ projectId, pageSize: 500, page: 1 })
        .pipe(toHttpException()),
    );

    const declarers = new Map<string, { script: string; revision: string }>();
    for (const script of page.scripts || []) {
      if (!script.currentRevisionId) continue;
      const revision = await lastValueFrom(
        this.scriptService
          .getRevision({
            id: script.currentRevisionId,
            withBundle: false,
            manifestOnly: false,
          })
          .pipe(toHttpException()),
      );
      for (const name of revision.scriptConfig?.capabilities?.secrets ?? []) {
        declarers.set(name, {
          script: script.publicIdentifier,
          revision: script.currentRevisionId,
        });
      }
    }
    return declarers;
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
    const [response, declarers] = await Promise.all([
      lastValueFrom(
        this.secretService
          .listSecrets({ projectId: project.id })
          .pipe(toHttpException()),
      ),
      this.declarers(project.id),
    ]);

    const metas = response.secrets || [];
    const names = await this.usernames(metas.map((meta) => meta.createdBy));

    return metas.map(
      (meta) =>
        new SecretDto(
          meta,
          names.get(meta.createdBy) ?? '',
          declarers.get(meta.name) ?? null,
        ),
    );
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

    const [declarers, names] = await Promise.all([
      this.declarers(project.id),
      this.usernames([meta.createdBy]),
    ]);
    return new SecretDto(
      meta,
      names.get(meta.createdBy) ?? '',
      declarers.get(name) ?? null,
    );
  }

  /**
   * A name's rotation history, newest first: timestamps and authors
   * only. Old values are retained encrypted solely for workflow runs
   * that pinned them; nothing reads them back out here.
   */
  @Get(':name/versions')
  @AclByProject(AccessFields.SECRETS_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async listSecretVersions(
    @EntityParam('project', Projects) project: Projects,
    @Param('name') name: string,
  ): Promise<SecretVersionDto[]> {
    const response = await lastValueFrom(
      this.secretService
        .listSecretVersions({ projectId: project.id, name })
        .pipe(toHttpException()),
    );

    const rows = response.versions || [];
    const names = await this.usernames(rows.map((row) => row.createdBy));
    return rows.map(
      (row) => new SecretVersionDto(row, names.get(row.createdBy) ?? ''),
    );
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

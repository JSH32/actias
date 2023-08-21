import {
  Body,
  Controller,
  Delete,
  Get,
  Inject,
  OnModuleInit,
  Param,
  Patch,
  Post,
  Put,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { script_service } from 'src/protobufs/script_service';

import { toHttpException } from 'src/exceptions/grpc.exception';
import { ScriptDto } from './dto/script.dto';
import {
  CreateRevisionDto,
  CreateScriptDto,
  NewRevisionResponseDto,
} from './dto/requests.dto';
import { RevisionDataDto, RevisionFullDto } from './dto/revision.dto';
import {
  PaginatedResponseDto,
  ApiOkResponsePaginated,
} from 'src/shared/dto/paginated';
import { AuthGuard } from 'src/auth/auth.guard';
import {
  AclByProject,
  AclByResource,
  AclGuard,
} from 'src/project/acl/acl.guard';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { ProjectService } from 'src/project/project.service';
import { ResourceType } from 'src/entities/Resources';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityManager } from '@mikro-orm/postgresql';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('scripts')
@Controller('project/:project/scripts')
export class ProjectScriptController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    private readonly projectService: ProjectService,
  ) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Get a paginated list of scripts.
   */
  @Get()
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiOkResponsePaginated(ScriptDto)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async listScripts(
    @EntityParam('project', Projects) project: Projects,
    @Query('page')
    page: number,
  ): Promise<PaginatedResponseDto<ScriptDto>> {
    return await this.projectService.listResources(
      ResourceType.SCRIPT,
      project,
      page,
      (res) =>
        lastValueFrom(
          this.scriptService
            .queryScript({ id: res.serviceId })
            .pipe(toHttpException()),
        ).then((s) => new ScriptDto(res.id, project.id, s)),
    );
  }

  @Post()
  @AclByProject(AccessFields.SCRIPT_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async createScript(
    @EntityParam('project', Projects) project: Projects,
    @Body() createScript: CreateScriptDto,
  ): Promise<ScriptDto> {
    const rpcScript = await lastValueFrom(
      this.scriptService.createScript(createScript).pipe(toHttpException()),
    );

    const script = await this.projectService.createResource(
      project,
      ResourceType.SCRIPT,
      rpcScript.id,
    );

    return new ScriptDto(script.id, project.id, rpcScript);
  }
}

@UseGuards(AuthGuard, AclGuard)
@ApiTags('scripts')
@Controller('script')
export class ScriptsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    private readonly projectService: ProjectService,
    private readonly em: EntityManager,
  ) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Get a list of revisions (bundle not included).
   */
  @Get(':id/revisions')
  @AclByResource(AccessFields.SCRIPT_READ, 'id')
  @ApiOkResponsePaginated(RevisionDataDto)
  async revisionList(
    @Param('id') scriptId: string,
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<RevisionDataDto>> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      scriptId,
    );

    const revisionPage = await lastValueFrom(
      this.scriptService
        .listRevisions({
          scriptId: resource.serviceId,
          pageSize: 10,
          page: page - 1,
        })
        .pipe(toHttpException()),
    );

    return {
      lastPage: revisionPage.totalPages + 1,
      page: page,
      items: revisionPage.revisions
        ? revisionPage.revisions.map(
          (revision) => new RevisionFullDto(scriptId, revision),
        )
        : [],
    };
  }

  /**
   * Set the current active revision for a script.
   */
  @Patch(':id/revisions')
  @AclByResource(AccessFields.SCRIPT_WRITE, 'id')
  async setRevision(
    @Param('id') scriptId: string,
    @Query('revisionId') revisionId: string,
  ): Promise<NewRevisionResponseDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      scriptId,
    );

    return (await lastValueFrom(
      this.scriptService
        .setScriptRevision({
          scriptId: resource.serviceId,
          revisionId,
        })
        .pipe(toHttpException()),
    )) as NewRevisionResponseDto;
  }

  /**
   * Create a new revision.
   */
  @Put(':id/revisions')
  @AclByResource(AccessFields.SCRIPT_WRITE, 'id')
  async createRevision(
    @Param('id') scriptId: string,
    @Body()
    request: CreateRevisionDto,
  ): Promise<RevisionDataDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      scriptId,
    );

    return new RevisionFullDto(
      scriptId,
      await lastValueFrom(
        this.scriptService
          .createRevision({
            scriptId: resource.serviceId,
            bundle: request.bundle.toServiceBundle(),
            scriptConfig: JSON.stringify({
              ...request.scriptConfig,
              // Adapt the service ID.
              id: resource.serviceId,
            }),
          })
          .pipe(toHttpException()),
      ),
    );
  }

  /**
   * Get a script by ID.
   */
  @Get(':id')
  @AclByResource(AccessFields.SCRIPT_READ, 'id')
  async getScript(@Param('id') id: string): Promise<ScriptDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      id,
    );

    return new ScriptDto(
      resource.id,
      resource.project.id,
      await lastValueFrom(
        this.scriptService
          .queryScript({ id: resource.serviceId })
          .pipe(toHttpException()),
      ),
    );
  }

  @Delete(':id')
  @AclByResource(AccessFields.SCRIPT_WRITE, 'id')
  async deleteScript(@Param('id') scriptId: string) {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      scriptId,
    );

    await this.em.removeAndFlush(resource);

    return new MessageResponseDto('Successfully deleted script');
  }
}

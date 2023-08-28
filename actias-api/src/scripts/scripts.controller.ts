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
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
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
import { AclByFinder, AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { ProjectService } from 'src/project/project.service';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityManager } from '@mikro-orm/postgresql';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('scripts')
@ApiBearerAuth()
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
    const scripts = await lastValueFrom(
      this.scriptService
        .listScripts({
          page: page - 1,
          pageSize: 25,
          projectId: project.id,
        })
        .pipe(toHttpException()),
    );

    return PaginatedResponseDto.fromArray(
      page,
      scripts.totalPages,
      (scripts.scripts || []).map((script) => new ScriptDto(script)),
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
    const script = await lastValueFrom(
      this.scriptService
        .createScript({
          publicIdentifier: createScript.publicIdentifier,
          projectId: project.id,
        })
        .pipe(toHttpException()),
    );

    return new ScriptDto(script);
  }
}

@UseGuards(AuthGuard, AclGuard)
@ApiTags('scripts')
@ApiBearerAuth()
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

  async projectFinder(request: any, em: EntityManager) {
    const script = await lastValueFrom(
      this.scriptService
        .queryScript({ id: request.params['id'] })
        .pipe(toHttpException()),
    );

    return await em.findOneOrFail(Projects, { id: script.projectId });
  }

  /**
   * Get a list of revisions (bundle not included).
   */
  @Get(':id/revisions')
  @AclByFinder(AccessFields.SCRIPT_READ, 'projectFinder')
  @ApiOkResponsePaginated(RevisionDataDto)
  async revisionList(
    @Param('id') scriptId: string,
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<RevisionDataDto>> {
    const revisionPage = await lastValueFrom(
      this.scriptService
        .listRevisions({
          scriptId,
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
            (revision) => new RevisionFullDto(revision),
          )
        : [],
    };
  }

  /**
   * Set the current active revision for a script.
   */
  @Patch(':id/revisions')
  @AclByFinder(AccessFields.SCRIPT_WRITE, 'projectFinder')
  async setRevision(
    @Param('id') scriptId: string,
    @Query('revisionId') revisionId: string,
  ): Promise<NewRevisionResponseDto> {
    return (await lastValueFrom(
      this.scriptService
        .setScriptRevision({ scriptId, revisionId })
        .pipe(toHttpException()),
    )) as NewRevisionResponseDto;
  }

  /**
   * Create a new revision.
   */
  @Put(':id/revisions')
  @AclByFinder(AccessFields.SCRIPT_WRITE, 'projectFinder')
  async createRevision(
    @Param('id') scriptId: string,
    @Body()
    request: CreateRevisionDto,
  ): Promise<RevisionDataDto> {
    return new RevisionFullDto(
      await lastValueFrom(
        this.scriptService
          .createRevision({
            scriptId,
            bundle: request.bundle.toServiceBundle(),
            scriptConfig: JSON.stringify({
              ...request.scriptConfig,
              // Adapt the service ID.
              id: scriptId,
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
  @AclByFinder(AccessFields.SCRIPT_READ, 'projectFinder')
  async getScript(@Param('id') scriptId: string): Promise<ScriptDto> {
    return new ScriptDto(
      await lastValueFrom(
        this.scriptService
          .queryScript({ id: scriptId })
          .pipe(toHttpException()),
      ),
    );
  }

  @Delete(':id')
  @AclByFinder(AccessFields.SCRIPT_WRITE, 'projectFinder')
  async deleteScript(@Param('id') scriptId: string) {
    await lastValueFrom(
      this.scriptService.deleteScript({ scriptId }).pipe(toHttpException()),
    );

    return new MessageResponseDto('Successfully deleted script');
  }
}

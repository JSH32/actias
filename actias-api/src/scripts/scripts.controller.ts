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
import { Acl, AclGuard } from 'src/project/acl/acl.guard';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { ProjectService } from 'src/project/project.service';
import { ResourceType } from 'src/entities/Resources';
import { AccessFields } from 'src/project/acl/accessFields';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('scripts')
@Controller('project/:project/script')
export class ScriptsController implements OnModuleInit {
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
   * Get a list of revisions (bundle not included).
   */
  @Get(':id/revisions')
  @Acl(AccessFields.SCRIPT_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  @ApiOkResponsePaginated(RevisionDataDto)
  async revisionList(
    @EntityParam('project', Projects) project: Projects,
    @Param('id') scriptId: string,
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<RevisionDataDto>> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      project,
      scriptId,
    );

    const revisionPage = await lastValueFrom(
      this.scriptService
        .listRevisions({
          scriptId: resource.serviceId,
          pageSize: 10,
          page: page,
        })
        .pipe(toHttpException()),
    );

    return {
      lastPage: revisionPage.totalPages,
      page: revisionPage.page,
      items: revisionPage.revisions.map(
        (revision) => new RevisionFullDto(revision),
      ),
    };
  }

  /**
   * Set the current active revision for a script.
   */
  @Patch(':id/revisions')
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  @Acl(AccessFields.SCRIPT_WRITE)
  async setRevision(
    @EntityParam('project', Projects) project: Projects,
    @Param('id') scriptId: string,
    @Query('revisionId') revisionId: string,
  ): Promise<NewRevisionResponseDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      project,
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
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  @Acl(AccessFields.SCRIPT_WRITE)
  async createRevision(
    @EntityParam('project', Projects) project: Projects,
    @Param('id') scriptId: string,
    @Body()
    request: CreateRevisionDto,
  ): Promise<RevisionDataDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      project,
      scriptId,
    );

    return new RevisionFullDto(
      await lastValueFrom(
        this.scriptService
          .createRevision({
            scriptId: resource.serviceId,
            bundle: request.bundle.toServiceBundle(),
            projectConfig: JSON.stringify(request.projectConfig),
          })
          .pipe(toHttpException()),
      ),
    );
  }

  /**
   * Get a paginated list of scripts.
   */
  @Get('/list')
  @Acl(AccessFields.SCRIPT_READ)
  @ApiOkResponsePaginated(ScriptDto)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  async listScripts(
    @EntityParam('project', Projects) project: Projects,
    @Query('page') page: number,
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
        ).then((s) => new ScriptDto(res.id, s)),
    );
  }

  /**
   * Get a script by ID.
   */
  @Get(':id')
  @Acl(AccessFields.SCRIPT_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  async getScript(
    @EntityParam('project', Projects) project: Projects,
    @Param('id') id: string,
  ): Promise<ScriptDto> {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      project,
      id,
    );

    return new ScriptDto(
      resource.id,
      await lastValueFrom(
        this.scriptService
          .queryScript({ id: resource.serviceId })
          .pipe(toHttpException()),
      ),
    );
  }

  @Delete(':script')
  @Acl(AccessFields.SCRIPT_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  async deleteScript(
    @EntityParam('project', Projects) project: Projects,
    @Param('script') scriptId: string,
  ) {
    const resource = await this.projectService.getResource(
      ResourceType.SCRIPT,
      project,
      scriptId,
    );

    await lastValueFrom(
      this.scriptService
        .deleteScript({ scriptId: resource.serviceId })
        .pipe(toHttpException()),
    );
  }

  @Post()
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
  })
  @Acl(AccessFields.SCRIPT_WRITE)
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

    return new ScriptDto(
      script.id,
      await lastValueFrom(
        this.scriptService.createScript(createScript).pipe(toHttpException()),
      ),
    );
  }
}

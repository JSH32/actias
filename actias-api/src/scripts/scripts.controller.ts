import {
  Body,
  Controller,
  Delete,
  Get,
  HttpException,
  HttpStatus,
  Inject,
  OnModuleInit,
  Param,
  Patch,
  Post,
  Put,
  Query,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiTags } from '@nestjs/swagger';
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

@ApiTags('scripts')
@Controller('scripts')
export class ScriptsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(@Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Get a list of revisions (bundle not included).
   */
  @Get(':id/revisions')
  @ApiOkResponsePaginated(RevisionDataDto)
  async revisionList(
    @Param('id') scriptId: string,
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<RevisionDataDto>> {
    const response = await lastValueFrom(
      this.scriptService
        .listRevisions({
          scriptId,
          pageSize: 10,
          page: page - 1,
        })
        .pipe(toHttpException()),
    );

    if (!response.revisions) {
      throw new HttpException(
        `Invalid page, last page is ${response.totalPages + 1}`,
        HttpStatus.BAD_REQUEST,
      );
    }

    return {
      page: response.page + 1,
      totalPages: response.totalPages + 1,
      items: response.revisions.map(
        (revision) => new RevisionFullDto(revision),
      ),
    };
  }

  @Patch(':id/revisions')
  async setRevision(
    @Param('id') scriptId: string,
    @Query('revisionId') revisionId: string,
  ): Promise<NewRevisionResponseDto> {
    return (await lastValueFrom(
      this.scriptService
        .setScriptRevision({
          scriptId,
          revisionId,
        })
        .pipe(toHttpException()),
    )) as NewRevisionResponseDto;
  }

  /**
   * Create a new revision.
   */
  @Put(':id/revisions')
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
  @ApiOkResponsePaginated(ScriptDto)
  async listScripts(
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<ScriptDto>> {
    const response = await lastValueFrom(
      this.scriptService
        .listScripts({
          pageSize: 25,
          page: page - 1,
        })
        .pipe(toHttpException()),
    );

    if (!response.scripts) {
      throw new HttpException(
        `Invalid page, last page is ${response.totalPages + 1}`,
        HttpStatus.BAD_REQUEST,
      );
    }

    return {
      page: response.page + 1,
      totalPages: response.totalPages + 1,
      items: response.scripts.map((script) => new ScriptDto(script)),
    };
  }

  /**
   * Get a script by ID.
   */
  @Get(':id')
  async getScript(@Param('id') id: string): Promise<ScriptDto> {
    return new ScriptDto(
      await lastValueFrom(
        this.scriptService.queryScript({ id }).pipe(toHttpException()),
      ),
    );
  }

  @Delete(':id')
  async deleteScript(@Param('id') scriptId: string) {
    await lastValueFrom(
      this.scriptService.deleteScript({ scriptId }).pipe(toHttpException()),
    );
  }

  @Post()
  async createScript(
    @Body() createScript: CreateScriptDto,
  ): Promise<ScriptDto> {
    return new ScriptDto(
      await lastValueFrom(
        this.scriptService.createScript(createScript).pipe(toHttpException()),
      ),
    );
  }
}

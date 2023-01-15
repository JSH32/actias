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
  Query,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiQuery, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { script_service } from 'src/protobufs/script_service';

import { toHttpException } from 'src/exceptions/grpc.exception';
import { ScriptDto } from './dto/script.dto';
import {
  CreateRevisionDto,
  CreateScriptDto,
  RevisionRequestTypeDto,
  toRevisionNum,
} from './dto/requests.dto';
import { RevisionDto } from './dto/revision.dto';
import { BundleDto } from './dto/bundle.dto';

@ApiTags('scripts')
@Controller('scripts')
export class ScriptsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(@Inject('SCRIPT_SERVICE') private client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  @Get()
  @ApiQuery({
    name: 'revisions',
    enum: RevisionRequestTypeDto,
    required: false,
  })
  @ApiQuery({ name: 'id', required: false })
  @ApiQuery({ name: 'publicName', required: false })
  async getScript(
    @Query('id') id?: string,
    @Query('publicName') publicName?: string,
    @Query('revisions')
    revisions?: RevisionRequestTypeDto,
  ): Promise<ScriptDto> {
    if (!id && !publicName) {
      throw new HttpException(
        "Must have either 'id' or 'publicName' parameter",
        HttpStatus.BAD_REQUEST,
      );
    }

    return new ScriptDto(
      await lastValueFrom(
        this.scriptService
          .queryScript({
            id,
            publicName,
            revisionRequestType: toRevisionNum(
              revisions || RevisionRequestTypeDto.NONE,
            ),
          })
          .pipe(toHttpException()),
      ),
    );
  }

  @Get('/list')
  async listScripts(@Query('page') page: number): Promise<ScriptDto[]> {
    const response = await lastValueFrom(
      this.scriptService
        .listScripts({
          pageSize: 25,
          page,
        })
        .pipe(toHttpException()),
    );

    return response.scripts.map((script) => new ScriptDto(script));
  }

  @Delete(':id')
  async deleteScript(@Param('id') scriptId: string) {
    this.scriptService.deleteScript({ scriptId }).pipe(toHttpException());
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

  /**
   * Get a list of revisions (bundle not included).
   */
  @Get(':id/revisions')
  async revisionList(
    @Param('id') scriptId: string,
    @Query('page') page: number,
  ): Promise<RevisionDto[]> {
    const response = await lastValueFrom(
      this.scriptService
        .listRevisions({
          scriptId,
          pageSize: 10,
          page,
        })
        .pipe(toHttpException()),
    );

    return response.revisions.map((revision) => new RevisionDto(revision));
  }

  /**
   * Create a new revision.
   */
  @Patch(':id/revisions')
  async createRevision(
    @Param('id') scriptId: string,
    @Body()
    request: CreateRevisionDto,
  ): Promise<RevisionDto> {
    console.log(new BundleDto(request.bundle).toServiceBundle());
    return new RevisionDto(
      await lastValueFrom(
        this.scriptService
          .createRevision({
            scriptId,
            bundle: new BundleDto(request.bundle).toServiceBundle(),
            projectConfig: JSON.stringify(request.projectConfig),
          })
          .pipe(toHttpException()),
      ),
    );
  }
}

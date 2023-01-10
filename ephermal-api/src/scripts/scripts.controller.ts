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
  Query,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiTags } from '@nestjs/swagger';
import { lastValueFrom, Observable } from 'rxjs';
import { script_service } from 'src/protobufs/script_service';
// eslint-disable-next-line @typescript-eslint/no-unused-vars
import { bundle } from 'src/protobufs/shared/bundle';

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
  async getScript(
    @Query('id') id?: string,
    @Query('publicName') publicName?: string,
    @Query('revisions')
    revisions?: script_service.FindScriptRequest.RevisionRequestType,
  ): Promise<script_service.Script> {
    if (!id && !publicName) {
      throw new HttpException(
        "Must have either 'id' or 'publicName' parameter",
        HttpStatus.BAD_REQUEST,
      );
    }

    return await lastValueFrom(
      this.scriptService.queryScript({
        id,
        publicName,
        revisionRequestType:
          revisions ||
          script_service.FindScriptRequest.RevisionRequestType.NONE,
      }),
    );
  }

  @Get('/list')
  async listScripts(
    @Query('page') page: number,
  ): Promise<script_service.Script[]> {
    const response = await lastValueFrom(
      this.scriptService.listScripts({
        pageSize: 25,
        page,
      }),
    );

    return response.scripts;
  }

  @Delete(':id')
  async deleteScript(@Param('id') scriptId: string) {
    try {
      lastValueFrom(this.scriptService.deleteScript({ scriptId }));
      return { message: `Successfully deleted script ${scriptId}` };
    } catch (e) {
      throw new HttpException(e.toString(), HttpStatus.BAD_REQUEST);
    }
  }

  @Patch(':id/revision')
  createRevision(
    @Param('id') scriptId: string,
    @Body() bundle: bundle.Bundle,
  ): Observable<script_service.Revision> {
    return this.scriptService.createRevision({
      scriptId,
      bundle,
    });
  }
}

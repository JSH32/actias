import {
  Controller,
  Delete,
  Get,
  Inject,
  OnModuleInit,
  Param,
  Query,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { script_service } from 'src/protobufs/script_service';

@ApiTags('revisions')
@Controller('revisions')
export class RevisionsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(@Inject('SCRIPT_SERVICE') private client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  @Get('/list')
  async revisionList(
    @Query('page') page: number,
    @Query('script') scriptId?: string,
  ) {
    const response = await lastValueFrom(
      this.scriptService.listRevisions({
        pageSize: 10,
        page,
        scriptId,
      }),
    );

    return response.revisions.map((revision) => ({
      ...revision,
      projectConfig: JSON.parse(revision.projectConfig),
    }));
  }

  @Get(':id')
  async getRevision(@Param('id') id: string) {
    return await lastValueFrom(this.scriptService.getRevision({ id })).then(
      (revision) => ({
        ...revision,
        projectConfig: JSON.parse(revision.projectConfig),
      }),
    );
  }

  @Delete(':id')
  async deleteRevision(@Param('id') id: string) {
    return this.scriptService.deleteRevision({ id });
  }
}

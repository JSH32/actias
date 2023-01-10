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
    return this.scriptService.listRevisions({
      pageSize: 10,
      page,
      scriptId,
    });
  }

  @Get(':id')
  async getRevision(@Param('id') id: string) {
    return this.scriptService.getRevision({ id });
  }

  @Delete(':id')
  async deleteRevision(@Param('id') id: string) {
    return this.scriptService.deleteRevision({ id });
  }
}

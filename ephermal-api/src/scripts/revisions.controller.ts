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
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { RevisionFullDto } from './dto/revision.dto';

@ApiTags('revisions')
@Controller('revisions')
export class RevisionsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(@Inject('SCRIPT_SERVICE') private client: ClientGrpc) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Get a revision by ID.
   */
  @Get(':id')
  async getRevision(
    @Param('id') id: string,
    @Query('withBundle') withBundle?: boolean,
  ): Promise<RevisionFullDto> {
    return await lastValueFrom(
      this.scriptService
        .getRevision({ id, withBundle: withBundle || false })
        .pipe(toHttpException()),
    ).then((revision) => new RevisionFullDto(revision));
  }

  /**
   * Delete a revision by ID.
   */
  @Delete(':id')
  async deleteRevision(@Param('id') id: string) {
    return this.scriptService.deleteRevision({ id }).pipe(toHttpException());
  }
}

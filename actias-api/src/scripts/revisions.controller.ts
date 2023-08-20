import {
  Controller,
  Delete,
  Get,
  Inject,
  OnModuleInit,
  Param,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import { NewRevisionResponseDto } from './dto/requests.dto';
import { RevisionFullDto } from './dto/revision.dto';
import { AuthGuard } from 'src/auth/auth.guard';
import { AclByFinder, AclGuard } from 'src/project/acl/acl.guard';
import { EntityManager } from '@mikro-orm/core';
import { ResourceType, Resources } from 'src/entities/Resources';
import { AccessFields } from 'src/project/acl/accessFields';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('revisions')
@Controller('revisions')
export class RevisionsController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    private readonly em: EntityManager,
  ) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  async checkPermissions(request: any, em: EntityManager) {
    const revision = await lastValueFrom(
      this.scriptService
        .getRevision({
          id: request.params['id'],
          withBundle: false,
        })
        .pipe(toHttpException()),
    );

    return (
      await em.findOneOrFail(Resources, {
        serviceId: revision.scriptId,
      })
    ).project;
  }

  /**
   * Get a revision by ID.
   */
  @Get(':id')
  @AclByFinder(AccessFields.SCRIPT_READ, 'checkPermissions')
  async getRevision(
    @Param('id') id: string,
    @Query('withBundle') withBundle?: boolean,
  ): Promise<RevisionFullDto> {
    console.log(withBundle);
    const revision = await lastValueFrom(
      this.scriptService
        .getRevision({ id, withBundle: withBundle || false })
        .pipe(toHttpException()),
    );

    const script = await this.em.findOneOrFail(Resources, {
      resourceType: ResourceType.SCRIPT,
      serviceId: revision.scriptId,
    });

    return new RevisionFullDto(script.id, revision);
  }

  /**
   * Delete a revision by ID.
   */
  @Delete(':id')
  @AclByFinder(AccessFields.SCRIPT_WRITE, 'checkPermissions')
  async deleteRevision(
    @Param('id') id: string,
  ): Promise<NewRevisionResponseDto> {
    return (await lastValueFrom(
      this.scriptService
        .deleteRevision({ revisionId: id })
        .pipe(toHttpException()),
    )) as NewRevisionResponseDto;
  }
}

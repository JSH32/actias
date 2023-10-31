import {
  BadRequestException,
  Body,
  Controller,
  Delete,
  Get,
  Inject,
  Param,
  Put,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiBearerAuth, ApiParam, ApiQuery, ApiTags } from '@nestjs/swagger';
import { lastValueFrom } from 'rxjs';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { AccessFields } from 'src/project/acl/accessFields';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { kv_service } from 'src/protobufs/kv_service';
import { EntityParam } from 'src/util/entitydecorator';
import {
  ListNamespaceDto,
  NamespaceDto,
  PairDto,
  PairType,
} from './dto/responses.dto';
import { AuthGuard } from 'src/auth/auth.guard';
import { SetKeyDto } from './dto/requests.dto';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('kv')
@ApiBearerAuth()
@Controller('project/:project/kv')
export class KvController {
  private kvService: kv_service.KvService;
  constructor(@Inject('KV_SERVICE') private readonly client: ClientGrpc) {}

  onModuleInit() {
    this.kvService = this.client.getService<kv_service.KvService>('KvService');
  }

  /**
   * List all namespaces in a project.
   */
  @Get()
  @AclByProject(AccessFields.KV_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async listNamespaces(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<NamespaceDto[]> {
    return (
      await lastValueFrom(
        this.kvService
          .listNamespaces({ projectId: project.id })
          .pipe(toHttpException()),
      )
    ).namespaces as NamespaceDto[];
  }

  /**
   * Delete a namespace and all keys in a project.
   */
  @Delete(':namespace')
  @AclByProject(AccessFields.KV_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async deleteNamespace(
    @EntityParam('project', Projects) project: Projects,
    @Param('namespace') namespace: string,
  ) {
    await lastValueFrom(
      this.kvService
        .deleteNamespace({ projectId: project.id, namespace })
        .pipe(toHttpException()),
    );

    return new MessageResponseDto(`Namespace (${namespace}) deleted`);
  }

  /**
   * List pairs in a namespace.
   */
  @Get(':namespace')
  @AclByProject(AccessFields.KV_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  @ApiQuery({
    name: 'token',
    description: 'Pagination token',
    required: false,
    type: String,
  })
  async listNamespace(
    @EntityParam('project', Projects) project: Projects,
    @Param('namespace') namespace: string,
    @Query('token') token?: string,
  ): Promise<ListNamespaceDto> {
    const page = await lastValueFrom(
      this.kvService
        .listPairs({ projectId: project.id, namespace, token, pageSize: 25 })
        .pipe(toHttpException()),
    );

    return {
      pageSize: page.pageSize,
      token: page.token,
      pairs: (page.pairs || []).map(
        (item) =>
          new PairDto({
            ...item,
            type: PairType[item.type],
          }),
      ),
    };
  }

  /**
   * Get a value from a key in a namespace.
   */
  @Get(':namespace/:key')
  @AclByProject(AccessFields.KV_READ)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async getKey(
    @EntityParam('project', Projects) project: Projects,
    @Param('namespace') namespace: string,
    @Param('key') key: string,
  ): Promise<PairDto> {
    const pair = await lastValueFrom(
      this.kvService
        .getPair({ projectId: project.id, namespace, key })
        .pipe(toHttpException()),
    );
    return {
      ...(pair as unknown as PairDto),
      type: PairType[pair.type],
    };
  }

  /**
   * Delete a value from a key in a namespace.
   */
  @Delete(':namespace/:key')
  @AclByProject(AccessFields.KV_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async deleteKey(
    @EntityParam('project', Projects) project: Projects,
    @Param('namespace') namespace: string,
    @Param('key') key: string,
  ): Promise<MessageResponseDto> {
    await lastValueFrom(
      this.kvService
        .deletePairs({ pairs: [{ projectId: project.id, namespace, key }] })
        .pipe(toHttpException()),
    );

    return new MessageResponseDto(
      `${key} on ${namespace} deleted successfully`,
    );
  }

  /**
   * Create or update a value from a key in a namespace.
   */
  @Put(':namespace/:key')
  @AclByProject(AccessFields.KV_WRITE)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async setKey(
    @EntityParam('project', Projects) project: Projects,
    @Param('namespace') namespace: string,
    @Param('key') key: string,
    @Body() body: SetKeyDto,
  ): Promise<MessageResponseDto> {
    // Validate type provided with value.
    switch (body.type.toLowerCase()) {
      case 'json':
        try {
          JSON.parse(body.value);
        } catch (e) {
          throw new BadRequestException('Not a JSON value');
        }
        break;
      case 'integer':
        {
          if (Number.isNaN(parseInt(body.value, 10))) {
            throw new BadRequestException('Not an integer value');
          } else {
            (body as any).value = parseInt(body.value, 10);
          }
        }
        break;
      case 'number':
        if (!Number.isFinite(parseFloat(body.value))) {
          throw new BadRequestException('Not a number value');
        }
        break;
      case 'boolean':
        if (!['true', 'false'].includes(body.value)) {
          throw new BadRequestException('Not a boolean value');
        }
        break;
      case 'string':
        // Anything can be a string.
        break;
      default:
        throw new BadRequestException('Not a valid type');
    }

    await lastValueFrom(
      this.kvService
        .setPairs({
          pairs: [
            {
              projectId: project.id,
              namespace,
              key,
              value: body.value.toString(),
              type: PairType[body.type],
            },
          ],
        })
        .pipe(toHttpException()),
    );

    return new MessageResponseDto(`${key} on ${namespace} set successfully`);
  }
}

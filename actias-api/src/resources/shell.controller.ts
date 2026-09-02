import {
  Body,
  Controller,
  Inject,
  Logger,
  OnModuleInit,
  Post,
  UseGuards,
} from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { ClientGrpc } from '@nestjs/microservices';
import { lastValueFrom } from 'rxjs';
import { AuthGuard } from 'src/auth/auth.guard';
import { Principal } from 'src/auth/user.decorator';
import { AclByProject, AclGuard } from 'src/project/acl/acl.guard';
import { AccessFields } from 'src/project/acl/accessFields';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { kv_service } from 'src/protobufs/kv_service';
import { ResourcesService } from './resources.service';
import { ShellOutcomeDto, ShellRunDto } from './dto/resources.dto';

/**
 * The shell's escalation: a chunk of Luau run for real, on a worker, in
 * a fresh vm. The three read verbs and single method calls resolve in
 * the client; this is for the loop someone pasted in from a file. It
 * is deliberately not encouraged and deliberately possible.
 *
 * The chunk binds resources by the operator's principal, not by a
 * contract: a shell session publishes nothing, so this route derives
 * the grants from what the project holds, and the worker checks the
 * chunk against exactly those as if they were a contract. Write mode
 * on the client side gates it; every run is logged against the
 * account.
 */
@UseGuards(AuthGuard, AclGuard)
@ApiTags('shell')
@Controller('project/:project/shell')
export class ShellController implements OnModuleInit {
  private readonly logger = new Logger(ShellController.name);
  private kv!: kv_service.KvService;

  constructor(
    private readonly resources: ResourcesService,
    @Inject('KV_SERVICE') private readonly kvClient: ClientGrpc,
  ) {}

  onModuleInit() {
    this.kv = this.kvClient.getService<kv_service.KvService>('KvService');
  }

  @Post('run')
  @AclByProject(AccessFields.DATABASE_WRITE)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async runShell(
    @EntityParam('project', Projects) project: Projects,
    @Body() body: ShellRunDto,
    @Principal()
    principal: {
      user?: { id: string; username?: string };
      serviceToken?: { id: string; name?: string };
    },
  ): Promise<ShellOutcomeDto> {
    const who = principal.user
      ? principal.user.username ?? principal.user.id
      : `service token ${
          principal.serviceToken?.name ?? principal.serviceToken?.id ?? ''
        }`;
    this.logger.log(
      `shell run by ${who} on ${project.id}: ${body.source.length} chars, ${body.write ? 'write' : 'read-only'}`,
    );
    const [namespaces, databases, classes] = await Promise.all([
      lastValueFrom(
        this.kv
          .listNamespaces({ projectId: project.id })
          .pipe(toHttpException()),
      ),
      this.resources.listResources(project, 'databases'),
      this.resources.classDeclarations(project),
    ]);
    const outcome = await this.resources.runShell(
      project,
      body.source,
      {
        kv: (namespaces.namespaces ?? []).map((n) => n.name),
        databases: databases.map((d) => d.name),
        objects: Array.from(classes.keys()),
      },
      body.wallSecs ?? 30,
      body.write ?? false,
    );
    return {
      valueJson: outcome.valueJson,
      output: outcome.output,
      error: outcome.error || undefined,
      work: outcome.work,
      wallMs: outcome.wallMs,
    };
  }
}

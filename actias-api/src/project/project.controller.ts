import {
  BadRequestException,
  Body,
  Controller,
  Delete,
  Get,
  Inject,
  OnModuleInit,
  Patch,
  Post,
  Query,
  UseGuards,
  Req,
} from '@nestjs/common';
import { ClientGrpc } from '@nestjs/microservices';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { ConfigService } from '@nestjs/config';
import { Request } from 'express';
import { lastValueFrom } from 'rxjs';
import { toHttpException } from 'src/exceptions/grpc.exception';
import { script_service } from 'src/protobufs/script_service';
import {
  ProjectMoveDto,
  ProjectPolicyDto,
  ProjectPolicyViewDto,
  SetProjectRegionDto,
} from './dto/policy.dto';
import { CreateProjectDto } from './dto/requests.dto';
import { ProjectService } from './project.service';
import { ProjectDto } from './dto/project.dto';
import { AclByProject, AclGuard, AclMember } from './acl/acl.guard';
import { AuthGuard } from 'src/auth/auth.guard';
import { Users } from 'src/entities/Users';
import { User } from 'src/auth/user.decorator';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { AccessFields } from './acl/accessFields';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('project')
@Controller('project')
@ApiBearerAuth()
export class ProjectController implements OnModuleInit {
  private scriptService: script_service.ScriptService;

  constructor(
    private readonly projectService: ProjectService,
    @Inject('SCRIPT_SERVICE') private readonly client: ClientGrpc,
    private readonly configService: ConfigService,
  ) {}

  onModuleInit() {
    this.scriptService =
      this.client.getService<script_service.ScriptService>('ScriptService');
  }

  /**
   * Create a project and return the data.
   */
  @Post()
  async createProject(
    @User() user: Users,
    @Body() createProject: CreateProjectDto,
    @Req() request: Request,
  ): Promise<ProjectDto> {
    const project = await this.projectService.createProject(
      user,
      createProject.name,
    );
    // The home: the regional ingress's region when it said one, else
    // this control plane's own. Recorded at creation so the home is a
    // fact of the project, not a default read back later.
    // Read off the request rather than declared as a parameter: the
    // header is the ingress's, not part of the api a client sees.
    const ingressRegion = request.headers['x-actias-region'];
    const region =
      typeof ingressRegion === 'string' &&
      /^[a-z0-9][a-z0-9-]{0,15}$/.test(ingressRegion)
        ? ingressRegion
        : this.configService.get<string>('region');
    await lastValueFrom(
      this.scriptService
        .setProjectRegion({ projectId: project.id, region })
        .pipe(toHttpException()),
    );
    return new ProjectDto(project);
  }

  /**
   * Get projects that a user has access to.
   */
  @Get()
  @ApiOkResponsePaginated(ProjectDto)
  async listProjects(
    @User() user,
    @Query('page')
    page: number,
  ): Promise<PaginatedResponseDto<ProjectDto>> {
    if (page < 1) {
      throw new BadRequestException('invalid page number provided!');
    }

    const projectPage = await this.projectService.getAll(user, 10, page);
    return new PaginatedResponseDto({
      ...projectPage,
      items: projectPage.items.map((item) => new ProjectDto(item)),
    });
  }

  /**
   * Get a project by its ID.
   */
  @Get(':project')
  @AclMember()
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async getProject(
    @EntityParam('project', Projects) project,
  ): Promise<ProjectDto> {
    return new ProjectDto(project);
  }

  /**
   * Delete a project by its ID.
   */
  /**
   * The project's runtime policy: rates and egress lists, the defaults
   * when none was set. Lives in the script service, which is what the
   * workers read it from.
   */
  @Get(':project/policy')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async getPolicy(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ProjectPolicyViewDto> {
    const policy = await lastValueFrom(
      this.scriptService
        .getProjectPolicy({ projectId: project.id })
        .pipe(toHttpException()),
    );
    return ProjectPolicyViewDto.fromProto(policy);
  }

  /**
   * Moves the project to another home: marks it moving, drains, copies
   * its objects between the regions' buckets, flips the home (FLEET.md
   * 6.3). Answers at once with the move to follow; both regions must be
   * registered.
   */
  @Patch(':project/region')
  @AclByProject(AccessFields.FULL)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async moveProject(
    @EntityParam('project', Projects) project: Projects,
    @Body() body: SetProjectRegionDto,
  ): Promise<ProjectMoveDto> {
    const move = await lastValueFrom(
      this.scriptService
        .moveProject({ projectId: project.id, region: body.region })
        .pipe(toHttpException()),
    );
    return ProjectMoveDto.fromProto(move);
  }

  /**
   * The project's latest move between homes; an empty step means it
   * never moved.
   */
  @Get(':project/move')
  @AclByProject(AccessFields.SCRIPT_READ)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async getMove(
    @EntityParam('project', Projects) project: Projects,
  ): Promise<ProjectMoveDto> {
    const move = await lastValueFrom(
      this.scriptService
        .getProjectMove({ projectId: project.id })
        .pipe(toHttpException()),
    );
    return ProjectMoveDto.fromProto(move);
  }

  /**
   * Replaces the project's runtime policy. Every field is set: a rate of
   * 0 is unbounded, an empty allow list admits everything not denied.
   */
  @Patch(':project/policy')
  @AclByProject(AccessFields.FULL)
  @ApiParam({ name: 'project', schema: { type: 'string' }, type: 'string' })
  async setPolicy(
    @EntityParam('project', Projects) project: Projects,
    @Body() policy: ProjectPolicyDto,
  ): Promise<ProjectPolicyViewDto> {
    const stored = await lastValueFrom(
      this.scriptService
        .setProjectPolicy({
          projectId: project.id,
          requestsPerSec: policy.requestsPerSec,
          workUnitsPerSec: policy.workUnitsPerSec,
          egressAllow: policy.egressAllow,
          egressDeny: policy.egressDeny,
        })
        .pipe(toHttpException()),
    );
    return ProjectPolicyViewDto.fromProto(stored);
  }

  // Destroying a project takes every grant in it, which in practice
  // means the owner (who bypasses) or a member trusted with all of it.
  @Delete(':project')
  @AclByProject(AccessFields.FULL)
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async deleteProject(
    @EntityParam('project', Projects) project,
  ): Promise<MessageResponseDto> {
    await this.projectService.deleteProject(project);
    return new MessageResponseDto(
      `Deleted project (${project.name}) successfully.`,
    );
  }
}

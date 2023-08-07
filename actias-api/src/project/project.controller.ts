import { Body, Controller, Get, Post, UseGuards } from '@nestjs/common';
import { ApiParam, ApiTags } from '@nestjs/swagger';
import { CreateProjectDto } from './dto/requests.dto';
import { ProjectService } from './project.service';
import { ProjectDto } from './dto/project.dto';
import { AclGuard } from './acl/acl.guard';
import { AuthGuard } from 'src/auth/auth.guard';
import { Users } from 'src/entities/Users';
import { User } from 'src/auth/user.decorator';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import { AclService } from './acl/acl.service';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('project')
@Controller('project')
export class ProjectController {
  constructor(
    private readonly projectService: ProjectService,
    private readonly accessService: AclService,
  ) {}

  /**
   * Create a project and return the data.
   */
  @Post()
  async createProject(
    @User() user: Users,
    @Body() createProject: CreateProjectDto,
  ): Promise<ProjectDto> {
    return new ProjectDto(
      await this.projectService.createProject(user.id, createProject.name),
    );
  }

  /**
   * Get a project by its ID.
   */
  @Get(':project')
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async get(
    @User() user,
    @EntityParam('project', Projects) project,
  ): Promise<ProjectDto> {
    await this.accessService.getProjectAccess(user, project);
    return new ProjectDto(project);
  }
}

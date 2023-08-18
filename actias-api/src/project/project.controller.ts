import {
  Body,
  Controller,
  Delete,
  Get,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
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
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';

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
      await this.projectService.createProject(user, createProject.name),
    );
  }

  /**
   * Get all projects that a user has access to.
   */
  @Get()
  @ApiOkResponsePaginated(ProjectDto)
  async getAll(
    @User() user,
    @Query('page')
    page: number,
  ): Promise<PaginatedResponseDto<ProjectDto>> {
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
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async get(@EntityParam('project', Projects) project): Promise<ProjectDto> {
    return new ProjectDto(project);
  }

  /**
   * Delete a project by its ID.
   */
  @Delete(':project')
  @ApiParam({
    name: 'project',
    schema: { type: 'string' },
    type: 'string',
  })
  async delete(
    @EntityParam('project', Projects) project,
  ): Promise<MessageResponseDto> {
    await this.projectService.delete(project);
    return new MessageResponseDto(
      `Deleted project (${project.name}) successfully.`,
    );
  }
}

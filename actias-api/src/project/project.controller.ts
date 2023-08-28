import {
  Body,
  Controller,
  Delete,
  Get,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ApiBearerAuth, ApiParam, ApiTags } from '@nestjs/swagger';
import { CreateProjectDto } from './dto/requests.dto';
import { ProjectService } from './project.service';
import { ProjectDto } from './dto/project.dto';
import { AclGuard } from './acl/acl.guard';
import { AuthGuard } from 'src/auth/auth.guard';
import { Users } from 'src/entities/Users';
import { User } from 'src/auth/user.decorator';
import { EntityParam } from 'src/util/entitydecorator';
import { Projects } from 'src/entities/Projects';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard, AclGuard)
@ApiTags('project')
@Controller('project')
@ApiBearerAuth()
export class ProjectController {
  constructor(private readonly projectService: ProjectService) {}

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
   * Get projects that a user has access to.
   */
  @Get()
  @ApiOkResponsePaginated(ProjectDto)
  async listProjects(
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
  async getProject(
    @EntityParam('project', Projects) project,
  ): Promise<ProjectDto> {
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
  async deleteProject(
    @EntityParam('project', Projects) project,
  ): Promise<MessageResponseDto> {
    await this.projectService.deleteProject(project);
    return new MessageResponseDto(
      `Deleted project (${project.name}) successfully.`,
    );
  }
}

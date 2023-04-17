import { Body, Controller, Get, Post, Req, UseGuards } from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { CreateProjectDto } from './dto/requests.dto';
import { ProjectService } from './project.service';
import { ProjectDto } from './dto/project.dto';
import { Acl } from './acl/acl.guard';
import { AccessFields } from './acl/accessFields';
import { AuthGuard } from 'src/auth/auth.guard';
import { Users } from 'src/entities/Users';

@UseGuards(AuthGuard)
@ApiTags('project')
@Controller('project')
export class ProjectController {
  constructor(private readonly projectService: ProjectService) {}

  @Post()
  async createProject(
    @Req() request: Request,
    @Body() createProject: CreateProjectDto,
  ): Promise<ProjectDto> {
    return new ProjectDto(
      await this.projectService.createProject(
        (request['user'] as Users).id,
        createProject.name,
      ),
    );
  }

  @Get(':project/test')
  @Acl(AccessFields.SCRIPT_READ)
  test() {
    return 'hi';
  }
}

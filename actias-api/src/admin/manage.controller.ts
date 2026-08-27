import {
  Body,
  Controller,
  Delete,
  Get,
  Param,
  ParseUUIDPipe,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ApiBearerAuth, ApiTags } from '@nestjs/swagger';
import { Admin, AuthGuard } from 'src/auth/auth.guard';
import { User } from 'src/auth/user.decorator';
import { Users } from 'src/entities/Users';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';
import { UserDto } from 'src/users/dto/user.dto';
import { AdminProjectDto, SetAdminDto } from './dto/manage.dto';
import { ManageService } from './manage.service';

@UseGuards(AuthGuard)
@ApiTags('admin')
@Controller('admin')
@ApiBearerAuth()
export class ManageController {
  constructor(private readonly manageService: ManageService) {}

  /**
   * Every user on the instance, newest first; search matches username
   * or email.
   */
  @Get('users')
  @Admin()
  @ApiOkResponsePaginated(UserDto)
  async listUsers(
    @Query('page') page: number,
    @Query('search') search?: string,
  ): Promise<PaginatedResponseDto<UserDto>> {
    return await this.manageService.listUsers(page, 25, search);
  }

  /**
   * Grants or revokes the admin flag; your own is another admin's to
   * change.
   */
  @Post('users/:user/admin')
  @Admin()
  async setUserAdmin(
    @User() actor: Users,
    @Param('user', new ParseUUIDPipe()) id: string,
    @Body() body: SetAdminDto,
  ): Promise<UserDto> {
    return await this.manageService.setAdmin(actor, id, body.admin);
  }

  /**
   * Deletes a user and every project they own, through the same
   * teardown an owner's own delete uses.
   */
  @Delete('users/:user')
  @Admin()
  async deleteUser(
    @User() actor: Users,
    @Param('user', new ParseUUIDPipe()) id: string,
  ): Promise<MessageResponseDto> {
    await this.manageService.deleteUser(actor, id);
    return new MessageResponseDto('User deleted.');
  }

  /**
   * Every project on the instance, newest first; search matches the
   * name.
   */
  @Get('projects')
  @Admin()
  @ApiOkResponsePaginated(AdminProjectDto)
  async listAllProjects(
    @Query('page') page: number,
    @Query('search') search?: string,
  ): Promise<PaginatedResponseDto<AdminProjectDto>> {
    return await this.manageService.listProjects(page, 25, search);
  }

  /**
   * Deletes any project, through the same teardown its owner would
   * trigger.
   */
  @Delete('projects/:project')
  @Admin()
  async deleteAnyProject(
    @Param('project', new ParseUUIDPipe()) id: string,
  ): Promise<MessageResponseDto> {
    const project = await this.manageService.deleteProject(id);
    return new MessageResponseDto(`Deleted project (${project.name}).`);
  }
}

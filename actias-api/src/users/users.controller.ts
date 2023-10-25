import {
  Body,
  Controller,
  Get,
  Post,
  Put,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ApiBearerAuth, ApiTags } from '@nestjs/swagger';
import {
  CreateUserDto,
  UpdatePasswordDto,
  UpdateUserDto,
} from './dto/requests.dto';
import { UserDto } from './dto/user.dto';
import { UsersService } from './users.service';
import { User } from 'src/auth/user.decorator';
import { AuthGuard, Public } from 'src/auth/auth.guard';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';

@ApiTags('users')
@Controller('users')
@UseGuards(AuthGuard)
export class UsersController {
  constructor(private readonly userService: UsersService) {}

  /**
   * Create a new user using standard username/password sign up.
   */
  @Post()
  @Public()
  async createUser(@Body() createUser: CreateUserDto): Promise<UserDto> {
    return new UserDto(await this.userService.createUser(createUser));
  }

  /**
   * Get the currently logged in user's details.
   */
  @Get('@me')
  @ApiBearerAuth()
  async me(@User() user): Promise<UserDto> {
    return new UserDto(user);
  }

  /**
   * Update user details.
   */
  @Put('@me/password')
  @ApiBearerAuth()
  async updatePassword(
    @User() user,
    @Body() updatePassword: UpdatePasswordDto,
  ): Promise<MessageResponseDto> {
    await this.userService.updatePassword(
      user,
      updatePassword.password,
      updatePassword.currentPassword,
    );
    return new MessageResponseDto('Password has been updated.');
  }

  /**
   * Update user details.
   */
  @Put('@me')
  @ApiBearerAuth()
  async update(
    @User() user,
    @Body() updateUser: UpdateUserDto,
  ): Promise<UserDto> {
    return new UserDto(await this.userService.updateUser(user, updateUser));
  }

  @Get()
  @ApiOkResponsePaginated(UserDto)
  @ApiBearerAuth()
  async searchUsers(
    @Query('name') name: string,
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<UserDto>> {
    const paginated = await this.userService.searchByQuery(page, 10, name);

    return {
      page: paginated.page,
      lastPage: paginated.lastPage,
      items: paginated.items.map((item) => new UserDto(item)),
    };
  }
}

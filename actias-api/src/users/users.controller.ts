import {
  Body,
  Controller,
  Get,
  Param,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { CreateUserDto } from './dto/requests.dto';
import { UserDto } from './dto/user.dto';
import { UsersService } from './users.service';
import { User } from 'src/auth/user.decorator';
import { AuthGuard, Public } from 'src/auth/auth.guard';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';

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
  async me(@User() user): Promise<UserDto> {
    return new UserDto(user);
  }

  @Get()
  @ApiOkResponsePaginated(UserDto)
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

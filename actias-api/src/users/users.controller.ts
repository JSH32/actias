import { Body, Controller, Get, Post, UseGuards } from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { CreateUserDto } from './dto/requests.dto';
import { UserDto } from './dto/user.dto';
import { UsersService } from './users.service';
import { User } from 'src/auth/user.decorator';
import { AuthGuard, Public } from 'src/auth/auth.guard';

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
}

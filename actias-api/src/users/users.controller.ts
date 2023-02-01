import { Body, Controller, Post } from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { CreateUserDto } from './dto/requests.dto';
import { UserDto } from './dto/user.dto';
import { UsersService } from './users.service';

@ApiTags('users')
@Controller('users')
export class UsersController {
  constructor(private readonly userService: UsersService) {}

  /**
   * Create a new user using standard username/password sign up.
   */
  @Post()
  async createUser(@Body() createUser: CreateUserDto): Promise<UserDto> {
    return new UserDto(await this.userService.createUser(createUser));
  }
}

import {
  Body,
  Controller,
  Get,
  NotFoundException,
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
import { PublicUserDto, UserDto } from './dto/user.dto';
import { UsersService } from './users.service';
import { User } from 'src/auth/user.decorator';
import { AuthGuard, Public } from 'src/auth/auth.guard';
import { MessageResponseDto } from 'src/shared/dto/message';
import { RegistrationConfigDto } from './dto/responses.dto';

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

  @Get('/registrationConfig')
  @Public()
  registrationConfig(): RegistrationConfigDto {
    return new RegistrationConfigDto({
      inviteOnly: this.userService.isInviteOnly(),
    });
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

  /**
   * Resolve one account by its exact email or username, for adding a
   * member to a project. Nothing here lists or matches loosely: browsing
   * the user table is an admin capability, at `GET /admin/users`.
   */
  @Get('lookup')
  @ApiBearerAuth()
  async lookupUser(
    @Query('identifier') identifier: string,
  ): Promise<PublicUserDto> {
    const user = await this.userService.findByIdentifier(identifier ?? '');
    if (!user) {
      throw new NotFoundException('No account with that email or username.');
    }
    return new PublicUserDto(user);
  }
}

import { Controller, Post, Body, UnauthorizedException } from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { AuthService } from './auth.service';
import { LoginDto } from './dto/requests.dto';
import { AuthTokenDto } from './dto/responses.dto';

@Controller('auth')
@ApiTags('auth')
export class AuthController {
  constructor(private authService: AuthService) {}

  /**
   * Login with username/password and recieve an authentication token.
   */
  @Post('login')
  async login(@Body() login: LoginDto): Promise<AuthTokenDto> {
    const user = await this.authService.passwordVerify(
      login.auth,
      login.password,
    );

    if (!user) throw new UnauthorizedException('Invalid username/password.');

    return {
      token: this.authService.signJwt(user, login.rememberMe),
    };
  }
}

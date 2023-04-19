import { HttpException, HttpStatus, Injectable } from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';
import { UsersService } from 'src/users/users.service';
import * as argon2 from 'argon2';
import { Users } from 'src/entities/Users';
import { AuthMethod } from 'src/entities/UserAuthMethod';

@Injectable()
export class AuthService {
  constructor(
    private readonly usersService: UsersService,
    private readonly jwtService: JwtService,
  ) {}

  /**
   * Verify user credentials.
   * This returns a User if the credentials are verified. Otherwise null.
   */
  async passwordVerify(auth: string, pass: string): Promise<Users | null> {
    const user = await this.usersService.findByAuth(auth);
    const password = user.authMethods.find(
      (method) => method.method === AuthMethod.PASSWORD,
    );

    if (!password) {
      throw new HttpException(
        "User isn't able to login with a password",
        HttpStatus.BAD_REQUEST,
      );
    }

    return (await argon2.verify(password.value, pass)) ? user : null;
  }

  signJwt(user: Users): string {
    console.log(this.jwtService);
    return this.jwtService.sign({ sub: user.id });
  }

  async getUserFromToken(token: string): Promise<Users> {
    const { sub } = await this.jwtService.verifyAsync(token);
    return this.usersService.findById(sub);
  }
}

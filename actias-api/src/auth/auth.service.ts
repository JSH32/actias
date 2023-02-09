import { HttpException, HttpStatus, Injectable } from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';
import { AuthMethod } from 'src/entities/UserAuthMethods';
import { UsersService } from 'src/users/users.service';
import * as argon2 from 'argon2';
import { Users } from 'src/entities/Users';

@Injectable()
export class AuthService {
  constructor(
    private readonly usersService: UsersService,
    private readonly jwtService: JwtService,
  ) {}

  async passwordVerify(auth: string, pass: string): Promise<Users | null> {
    const user = await this.usersService.findByAuth(auth);
    const password = user.authMethods
      .toArray()
      .find((method) => method.authMethod === AuthMethod.PASSWORD);

    if (!password) {
      throw new HttpException(
        "User isn't able to login with a password",
        HttpStatus.BAD_REQUEST,
      );
    }

    return (await argon2.verify(password.value, pass)) ? user : null;
  }

  getJwt(user: Users): string {
    return this.jwtService.sign({ sub: user.id });
  }
}

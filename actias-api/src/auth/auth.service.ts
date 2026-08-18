import {
  HttpException,
  HttpStatus,
  Injectable,
  UnauthorizedException,
} from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';
import { UsersService } from 'src/users/users.service';
import * as argon2 from 'argon2';
import { createHash } from 'crypto';
import { Users } from 'src/entities/Users';
import { AuthMethod } from 'src/entities/UserAuthMethod';
import { ServiceTokens } from 'src/entities/ServiceTokens';
import { EntityManager } from '@mikro-orm/postgresql';

/**
 * Every service token starts with this, so the auth path can tell a machine
 * credential from a user jwt without parsing either.
 */
export const SERVICE_TOKEN_PREFIX = 'actias_';

@Injectable()
export class AuthService {
  constructor(
    private readonly usersService: UsersService,
    private readonly jwtService: JwtService,
    private readonly em: EntityManager,
  ) {}

  /**
   * Verify user credentials.
   * This returns a User if the credentials are verified. Otherwise null.
   */
  async passwordVerify(auth: string, pass: string): Promise<Users | null> {
    const user = await this.usersService.findByAuth(auth);
    const password = user.authMethods
      .getItems()
      .find((method) => method.method === AuthMethod.PASSWORD);

    if (!password) {
      throw new HttpException(
        "User isn't able to login with a password",
        HttpStatus.BAD_REQUEST,
      );
    }

    return (await argon2.verify(password.value, pass)) ? user : null;
  }

  signJwt(user: Users, rememberMe = false): string {
    return this.jwtService.sign(
      { sub: user.id },
      { expiresIn: rememberMe ? '60d' : '1d' },
    );
  }

  async getUserFromToken(token: string): Promise<Users> {
    const { sub } = await this.jwtService.verifyAsync(token);
    return this.usersService.findById(sub);
  }

  /**
   * Resolves a service token by its hash; revocation is deletion, so a
   * revoked token simply does not resolve. Only the hash ever touches the
   * database.
   */
  async getServiceToken(token: string): Promise<ServiceTokens> {
    const tokenHash = createHash('sha256').update(token).digest('hex');

    const found = await this.em.findOne(
      ServiceTokens,
      { tokenHash },
      { populate: ['project'] },
    );
    if (!found) {
      throw new UnauthorizedException();
    }

    found.lastUsed = new Date();
    await this.em.flush();

    return found;
  }
}

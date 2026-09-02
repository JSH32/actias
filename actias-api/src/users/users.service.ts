import { EntityManager } from '@mikro-orm/core';
import { BadRequestException, Injectable } from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { CreateUserDto, UpdateUserDto } from './dto/requests.dto';
import * as argon2 from 'argon2';
import { UserAuthMethod, AuthMethod } from 'src/entities/UserAuthMethod';
import { ConfigService } from '@nestjs/config';
import { RegistrationCodes } from 'src/entities/RegistrationCode';

@Injectable()
export class UsersService {
  constructor(
    private readonly em: EntityManager,
    private readonly config: ConfigService,
  ) {}

  /**
   * Find a user by either their email or username.
   * @param auth email or username
   * @returns user
   */
  async findByAuth(auth: string): Promise<Users> {
    return await this.em.findOneOrFail(
      Users,
      {
        $or: [{ email: auth }, { username: auth }],
      },
      { populate: ['authMethods'] },
    );
  }

  async updatePassword(
    user: Users,
    password: string,
    currentPassword?: string,
  ) {
    const authMethods = await user.authMethods.loadItems();

    for (const method of authMethods) {
      if (method.method === AuthMethod.PASSWORD) {
        if (!(await argon2.verify(method.value, currentPassword))) {
          throw new BadRequestException('Incorrect current password.');
        }

        method.value = await argon2.hash(password);
        await this.em.persistAndFlush(method);

        return;
      }
    }

    // Add new password auth method if not exist before.
    user.authMethods.add(
      new UserAuthMethod({
        value: await argon2.hash(password),
        method: AuthMethod.PASSWORD,
      }),
    );

    await this.em.persistAndFlush(user);
  }

  /**
   * The one account whose address or username IS what was typed, or null.
   *
   * Exact by construction: the identifier is escaped before it reaches
   * `$ilike`, so a caller cannot pass `%` and turn a lookup back into a
   * listing. Case is ignored because neither column is normalized on
   * the way in, and an inviter types what they were told, not what was
   * stored.
   */
  async findByIdentifier(identifier: string): Promise<Users | null> {
    const wanted = identifier.trim();
    if (!wanted) return null;

    const literal = wanted.replace(/[\\%_]/g, (char) => `\\${char}`);
    return await this.em.findOne(Users, {
      $or: [{ email: { $ilike: literal } }, { username: { $ilike: literal } }],
    });
  }

  async findById(id: string): Promise<Users> {
    return await this.em.findOneOrFail(Users, { id });
  }

  async updateUser(user: Users, updateUser: UpdateUserDto): Promise<Users> {
    const conditions = [];
    if (user.email != updateUser.email) {
      conditions.push({ email: updateUser.email });
    }

    if (user.username != updateUser.username) {
      conditions.push({ username: updateUser.username });
    }

    // Check if user exists.
    if (
      await this.em.findOne(Users, {
        $or: conditions,
      })
    ) {
      throw new BadRequestException(
        'User with that username/email already exists.',
      );
    }

    // Update fields
    user.username = updateUser.username;
    user.email = updateUser.email;

    await this.em.persistAndFlush(user);
    return user;
  }

  isInviteOnly(): boolean {
    return this.config.getOrThrow<boolean>('inviteOnly');
  }

  async createUser(createUser: CreateUserDto): Promise<Users> {
    // Check if user exists.
    if (
      await this.em.findOne(Users, {
        $or: [{ email: createUser.email }, { username: createUser.username }],
      })
    ) {
      throw new BadRequestException(
        'User with that username/email already exists.',
      );
    }

    if (this.isInviteOnly()) {
      if (!createUser.registrationCode) {
        throw new BadRequestException('Registration code is required.');
      }

      const code = await this.em.findOneOrFail(RegistrationCodes, {
        id: createUser.registrationCode,
      });

      if (code.uses <= 1) {
        this.em.removeAndFlush(code);
      } else {
        code.uses -= 1;
        this.em.persistAndFlush(code);
      }
    }

    const user = new Users({
      username: createUser.username,
      email: createUser.email,
    });

    user.authMethods.add(
      new UserAuthMethod({
        value: await argon2.hash(createUser.password),
        method: AuthMethod.PASSWORD,
      }),
    );

    await this.em.persistAndFlush(user);

    return user;
  }
}

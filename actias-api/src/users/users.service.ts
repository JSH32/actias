import { EntityManager } from '@mikro-orm/core';
import { BadRequestException, Injectable } from '@nestjs/common';
import { Users } from 'src/entities/Users';
import { CreateUserDto } from './dto/requests.dto';
import * as argon2 from 'argon2';
import { UserAuthMethod, AuthMethod } from 'src/entities/UserAuthMethod';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';

@Injectable()
export class UsersService {
  constructor(private readonly em: EntityManager) {}

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

  async searchByQuery(
    page: number,
    pageSize: number,
    query: string,
  ): Promise<PaginatedResponseDto<Users>> {
    const [users, count] = await this.em.findAndCount(
      Users,
      {
        $or: [
          { email: { $like: `%${query}%` } },
          { username: { $like: `%${query}%` } },
        ],
      },
      { limit: pageSize, offset: (page - 1) * pageSize },
    );

    return {
      items: users,
      lastPage: Math.ceil(count / pageSize),
      page,
    };
  }

  async findById(id: string): Promise<Users> {
    return await this.em.findOneOrFail(Users, { id });
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

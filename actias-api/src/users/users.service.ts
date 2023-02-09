import { EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable } from '@nestjs/common';
import { AuthMethod, UserAuthMethods } from 'src/entities/UserAuthMethods';
import { Users } from 'src/entities/Users';
import { CreateUserDto } from './dto/requests.dto';
import * as argon2 from 'argon2';

@Injectable()
export class UsersService {
  constructor(
    @InjectRepository(Users)
    private readonly userRepository: EntityRepository<Users>,
  ) {}

  /**
   * Find a user by either their email or username.
   * @param auth email or username
   * @returns user
   */
  async findByAuth(auth: string): Promise<Users> {
    return await this.userRepository.findOneOrFail({
      $or: [{ email: auth }, { username: auth }],
    });
  }

  async createUser(createUser: CreateUserDto): Promise<Users> {
    const user = new Users({
      username: createUser.username,
      email: createUser.email,
    });

    user.authMethods.add(
      new UserAuthMethods({
        value: await argon2.hash(createUser.password),
        authMethod: AuthMethod.PASSWORD,
      }),
    );

    await this.userRepository.persistAndFlush(user);

    return user;
  }
}

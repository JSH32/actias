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

  async createUser(createUser: CreateUserDto) {
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

    // this.userRepository

    await this.userRepository.persistAndFlush(user);

    // await this.entityManager.persistAndFlush([user]);

    return user;
  }
}

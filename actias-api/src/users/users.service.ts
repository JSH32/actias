import { EntityRepository } from '@mikro-orm/core';
import { InjectRepository } from '@mikro-orm/nestjs';
import { Injectable } from '@nestjs/common';
import { UserAuthMethods } from 'src/entities/UserAuthMethods';
import { Users } from 'src/entities/Users';

@Injectable()
export class UsersService {
  constructor(
    @InjectRepository(Users)
    private readonly userRepository: EntityRepository<Users>,
    @InjectRepository(UserAuthMethods)
    private readonly authMethodRepository: EntityRepository<UserAuthMethods>,
  ) {}
}

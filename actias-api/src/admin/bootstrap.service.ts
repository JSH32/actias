import { EntityManager } from '@mikro-orm/core';
import { Injectable, Logger, OnApplicationBootstrap } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import * as argon2 from 'argon2';
import { AuthMethod, UserAuthMethod } from 'src/entities/UserAuthMethod';
import { Users } from 'src/entities/Users';

/**
 * First-run admin for self-hosted instances: when the ADMIN_* variables
 * are set and no user carries that username, one is created with the
 * admin flag on. An existing user by that name is promoted to admin but
 * never has its password touched, so the variables are safe to leave
 * set across restarts.
 */
@Injectable()
export class BootstrapService implements OnApplicationBootstrap {
  private readonly logger = new Logger(BootstrapService.name);

  constructor(
    private readonly em: EntityManager,
    private readonly config: ConfigService,
  ) {}

  async onApplicationBootstrap() {
    const { username, email, password } = this.config.get<{
      username?: string;
      email?: string;
      password?: string;
    }>('bootstrapAdmin');
    if (!username || !email || !password) return;

    const em = this.em.fork();
    const existing = await em.findOne(Users, { username });
    if (existing) {
      if (!existing.admin) {
        existing.admin = true;
        await em.persistAndFlush(existing);
        this.logger.log(`Existing user '${username}' promoted to admin.`);
      }
      return;
    }

    const user = new Users({ username, email });
    user.admin = true;
    user.authMethods.add(
      new UserAuthMethod({
        value: await argon2.hash(password),
        method: AuthMethod.PASSWORD,
      }),
    );
    await em.persistAndFlush(user);
    this.logger.log(`Bootstrap admin '${username}' created.`);
  }
}

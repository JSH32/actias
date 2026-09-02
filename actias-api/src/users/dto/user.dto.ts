import { Users } from 'src/entities/Users';

/**
 * A singular user.
 */
export class UserDto {
  /**
   * Users ID.
   */
  id!: string;

  /**
   * When the user was created.
   */
  created!: Date;

  /**
   * If the user is a system admin.
   */
  admin!: boolean;

  /**
   * Users email.
   */
  email!: string;

  /**
   * Users username.
   */
  username!: string;

  constructor(entity: Users) {
    return Object.assign(this, {
      id: entity.id,
      created: entity.createdAt,
      email: entity.email,
      admin: entity.admin,
      username: entity.username,
    });
  }
}

/**
 * A user as somebody who is not that user may see them: enough to
 * confirm you found the right person and to hand to an acl, and
 * nothing more. Addresses and the admin flag are deliberately absent,
 * so resolving an invitee never discloses either.
 */
export class PublicUserDto {
  /**
   * Users ID.
   */
  id!: string;

  /**
   * Users username.
   */
  username!: string;

  constructor(entity: Users) {
    return Object.assign(this, {
      id: entity.id,
      username: entity.username,
    });
  }
}

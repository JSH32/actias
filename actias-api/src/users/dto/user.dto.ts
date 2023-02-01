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
   * If the user is a systm admin.
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
      username: entity.username,
    });
  }
}

import { Users } from 'src/entities/Users';

type AuthMethods = 'discord' | 'google' | 'github';

export class UserDto {
  id!: string;
  created!: Date;
  email!: string;
  username!: string;
  // TODO: this entity
  authMethods!: AuthMethods[];

  // constructor(user: Users) {
  //   return Object.assign(this, {
  //     id: user.id,
  //     created: user.created,
  //     email: user.email,
  //     username: user.username,
  //     authMethods: user.authMethods.toArray(),
  //   });
  // }
}

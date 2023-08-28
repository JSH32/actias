import { UserDto } from 'src/users/dto/user.dto';

export class AclListDto {
  /**
   * User id that the permissions apply to.
   */
  user: UserDto;
  /**
   * List of permissions
   */
  permissions!: Record<string, boolean>;

  constructor(listData: Required<AclListDto>) {
    Object.assign(this, listData);
  }
}

export class AclListDto {
  /**
   * User id that the permissions apply to.
   */
  userId: string;
  /**
   * List of permissions
   */
  permissions!: Record<string, boolean>;

  constructor(listData: Required<AclListDto>) {
    Object.assign(this, listData);
  }
}

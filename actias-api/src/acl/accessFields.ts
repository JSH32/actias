/**
 * Access bitfield.
 */
export enum Access {
  NONE = 0,
  READ = 1 << 1,
  UPDATE = 1 << 2,
  CREATE = 1 << 3,
  DELETE = 1 << 4,
  MANAGE = READ | UPDATE | CREATE | DELETE,
}

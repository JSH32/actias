/**
 * Access bitfield.
 */
export enum AccessFields {
  NONE = 0,
  SCRIPT_READ = 1 << 1,
  SCRIPT_WRITE = 1 << 2,
  SCRIPT_RESOURCE = SCRIPT_READ | SCRIPT_WRITE,
  /**
   * All permissions for all resource types.
   */
  FULL = SCRIPT_RESOURCE,
}

import { BitField } from 'easy-bits';

/**
 * Access bitfield.
 */
export enum AccessFields {
  /**
   * Ability to modify scripts or publish/modify revisions.
   */
  SCRIPT_READ = 1 << 1,
  SCRIPT_WRITE = 1 << 2,
  SCRIPT_RESOURCE = SCRIPT_READ | SCRIPT_WRITE,
  /**
   * Ability to configure the project.
   * Such as renaming or adding members.
   */
  PERMISSIONS_READ = 1 << 3,
  PERMISSIONS_WRITE = 1 << 4,
  PERMISSIONS_RESOURCE = PERMISSIONS_READ | PERMISSIONS_WRITE,
  /**
   * All permissions for all resource types.
   */
  FULL = SCRIPT_RESOURCE | PERMISSIONS_RESOURCE,
}

export const ACCESS_KEYS = Object.keys(AccessFields).filter(
  (x) => !(parseInt(x) >= 0),
);

export const getListFromBitfield = (bitfield: string) => {
  const parsed = BitField.deserialize(bitfield);

  return Object.fromEntries(
    ACCESS_KEYS.map((key) => [key, parsed.test(AccessFields[key])]),
  );
};

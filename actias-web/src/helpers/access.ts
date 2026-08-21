/**
 * The grantable access bits: composite masks (SCRIPT_RESOURCE, FULL) are
 * derived conveniences in the api enum, not switches anyone flips. Every
 * surface that renders or edits bits filters through this.
 */
export function realBit(bit: string): boolean {
  return !bit.endsWith('_RESOURCE') && bit !== 'FULL';
}

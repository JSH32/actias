export class SetKeyDto {
  type: string;
  value: string;
}

/**
 * Value for a secret being set.
 */
export class SetSecretDto {
  /**
   * The plaintext value; it is encrypted before storage and never returned.
   */
  value: string;
}

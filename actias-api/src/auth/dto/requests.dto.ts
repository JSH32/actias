/**
 * Login using username/password to retrieve a token.
 */
export class LoginDto {
  /**
   * Either username or email.
   */
  auth!: string;
  password!: string;
}

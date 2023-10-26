/**
 * Login using username/password to retrieve a token.
 */
export class LoginDto {
  /**
   * Either username or email.
   */
  auth!: string;
  password!: string;
  /**
   * Should the generated token be remembered?
   * This changes expiration to 60 days from 1 day.
   */
  rememberMe?: boolean;
}

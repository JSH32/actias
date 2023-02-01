import { IsEmail, Length } from 'class-validator';

/**
 * Create a user through normal (non oauth) sign up.
 * This will create an auth method with the password type.
 */
export class CreateUserDto {
  /**
   * Username, must be unique.
   */
  @Length(6, 36)
  username!: string;

  @IsEmail()
  email!: string;

  /**
   * Password between 8 and 64 characters.
   */
  @Length(8, 64)
  password: string;
}

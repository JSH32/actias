import { IsEmail, IsUUID, Length } from 'class-validator';

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

  /**
   * Registration code (if needed).
   */
  @IsUUID()
  registrationCode?: string;
}

/**
 * Update user details.
 */
export class UpdateUserDto {
  @Length(6, 36)
  username!: string;

  @IsEmail()
  email!: string;
}

export class UpdatePasswordDto {
  /**
   * This is only needed if a password is set.
   * OAuth only accounts or accounts with alternative methods
   * do not need this.
   */
  currentPassword?: string;
  /**
   * Password between 8 and 64 characters.
   */
  @Length(8, 64)
  password: string;
}

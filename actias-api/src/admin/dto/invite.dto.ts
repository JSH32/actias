import { IsEmail } from 'class-validator';
import { RegistrationCodeDto } from './registration.dto';

/** Invite one person by address. */
export class InviteRequestDto {
  @IsEmail()
  email!: string;
}

export class InviteResponseDto {
  /** The one-use code backing the invite. */
  code!: RegistrationCodeDto;

  /** The register link the code rides in. */
  link!: string;

  /** Whether the invite was mailed; false means copy the link instead. */
  emailed!: boolean;

  constructor(code: RegistrationCodeDto, link: string, emailed: boolean) {
    this.code = code;
    this.link = link;
    this.emailed = emailed;
  }
}

/** What the admin surface adapts to. */
export class RegistrationSettingsDto {
  /** Whether this instance requires a code to register. */
  inviteOnly!: boolean;

  /** Whether invites can be mailed; false means links are copied. */
  smtpEnabled!: boolean;

  constructor(inviteOnly: boolean, smtpEnabled: boolean) {
    this.inviteOnly = inviteOnly;
    this.smtpEnabled = smtpEnabled;
  }
}

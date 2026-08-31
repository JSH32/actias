import { ApiProperty } from '@nestjs/swagger';
import { IsEmail, IsOptional, Length } from 'class-validator';
import { InterestSignup } from 'src/entities/InterestSignup';

/**
 * An address asking to hear when the project moves.
 */
export class CreateInterestDto {
  @IsEmail()
  @Length(3, 254)
  email!: string;

  /**
   * Which surface this came from. Defaults to `landing`, which is the
   * only one that asks today.
   */
  @IsOptional()
  @Length(1, 32)
  source?: string;
}

/**
 * One row of the list, for the instance admin reading it. The address
 * is the point of the record, so it is not redacted here; the endpoint
 * that returns it is admin-only.
 */
export class InterestSignupDto {
  @ApiProperty({ description: 'Signup id.' })
  id: string;

  @ApiProperty({ description: 'Address that asked to be kept posted.' })
  email: string;

  @ApiProperty({ description: 'Surface the address came from.' })
  source: string;

  @ApiProperty({ description: 'When the address was first recorded.' })
  createdAt: Date;

  constructor(signup: InterestSignup) {
    Object.assign(this, {
      id: signup.id,
      email: signup.email,
      source: signup.source,
      createdAt: signup.createdAt,
    });
  }
}

import { ApiProperty } from '@nestjs/swagger';
import { IsNotEmpty, IsString } from 'class-validator';
import { secret_service } from 'src/protobufs/secret_service';

/** One live secret's metadata; values never appear in any response. */
export class SecretDto {
  @ApiProperty()
  name: string;

  @ApiProperty({
    description: 'Head version; every rotation increments it.',
  })
  version: number;

  @ApiProperty({ description: 'Unix milliseconds of the head version write.' })
  createdMs: number;

  @ApiProperty({ description: 'User id that wrote the head version.' })
  createdBy: string;

  constructor(meta: secret_service.SecretMeta) {
    this.name = meta.name;
    // int64 fields arrive as proto-loader Longs; Number() reads them.
    this.version = Number(meta.version ?? 0);
    this.createdMs = Number(meta.createdMs ?? 0);
    this.createdBy = meta.createdBy || '';
  }
}

/** Value for a secret being set or rotated. */
export class SetSecretDto {
  @ApiProperty({
    description:
      'The plaintext value; encrypted at rest by the secret service and never returned.',
  })
  @IsString()
  @IsNotEmpty()
  value: string;
}

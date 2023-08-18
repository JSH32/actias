import { ApiProperty } from '@nestjs/swagger';

export class MessageResponseDto {
  @ApiProperty({ description: 'Message for the response' })
  message: string;

  constructor(message: string) {
    Object.assign(this, { message });
  }
}

import {
  Body,
  Controller,
  Delete,
  Get,
  Param,
  ParseUUIDPipe,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ConfigService } from '@nestjs/config';
import { ApiBearerAuth, ApiTags } from '@nestjs/swagger';
import { Admin, AuthGuard } from 'src/auth/auth.guard';
import { RegistrationService } from './registration.service';
import { RegistrationCodeDto } from './dto/registration.dto';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';
import {
  InviteRequestDto,
  InviteResponseDto,
  RegistrationSettingsDto,
} from './dto/invite.dto';
import { MailService } from './mail.service';

@UseGuards(AuthGuard)
@ApiTags('admin')
@Controller('admin/registration')
@ApiBearerAuth()
export class RegistrationController {
  constructor(
    private readonly registrationService: RegistrationService,
    private readonly mail: MailService,
    private readonly config: ConfigService,
  ) {}

  /**
   * What the admin surface adapts to: invite-only and mail capability.
   */
  @Get('settings')
  @Admin()
  registrationSettings(): RegistrationSettingsDto {
    return new RegistrationSettingsDto(
      this.config.getOrThrow<boolean>('inviteOnly'),
      this.mail.enabled,
    );
  }

  /**
   * Invite one person: a one-use code wrapped in a register link,
   * mailed when SMTP is configured, returned for copying either way.
   */
  @Post('invite')
  @Admin()
  async createInvite(
    @Body() invite: InviteRequestDto,
  ): Promise<InviteResponseDto> {
    const code = await this.registrationService.createRegistrationCode(1);
    const origin = this.config.get<string>('webOrigin') || '';
    const link = `${origin}/register?code=${code.id}`;

    let emailed = false;
    if (this.mail.enabled) {
      await this.mail.sendInvite(invite.email, link);
      emailed = true;
    }

    return new InviteResponseDto(code, link, emailed);
  }

  /**
   * Create a new registration code.
   */
  @Post()
  @Admin()
  async newRegistrationCode(
    @Query('uses') uses: number,
  ): Promise<RegistrationCodeDto> {
    return new RegistrationCodeDto(
      await this.registrationService.createRegistrationCode(uses),
    );
  }

  /**
   * List created registration codes.
   */
  @Get()
  @Admin()
  @ApiOkResponsePaginated(RegistrationCodeDto)
  async listRegistrationCodes(
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<RegistrationCodeDto>> {
    return await this.registrationService.listRegistrationCodes(page, 25);
  }

  @Delete(':code')
  @Admin()
  async deleteRegistrationCode(
    @Param('code', new ParseUUIDPipe()) code: string,
  ): Promise<MessageResponseDto> {
    await this.registrationService.deleteRegistrationCode(code);

    return new MessageResponseDto('Registration code deleted!');
  }
}

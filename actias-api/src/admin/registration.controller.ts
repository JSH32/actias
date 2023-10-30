import {
  Controller,
  Delete,
  Get,
  Param,
  ParseUUIDPipe,
  Post,
  Query,
  UseGuards,
} from '@nestjs/common';
import { ApiBearerAuth, ApiTags } from '@nestjs/swagger';
import { Admin, AuthGuard } from 'src/auth/auth.guard';
import { RegistrationService } from './registration.service';
import { RegistrationCodeDto } from './dto/registration.dto';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';

@UseGuards(AuthGuard)
@ApiTags('admin')
@Controller('admin/registration')
@ApiBearerAuth()
export class RegistrationController {
  constructor(private readonly registrationService: RegistrationService) {}

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

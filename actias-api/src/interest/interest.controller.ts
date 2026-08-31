import { Body, Controller, Get, Post, Query, UseGuards } from '@nestjs/common';
import { ApiBearerAuth, ApiTags } from '@nestjs/swagger';
import { Admin, AuthGuard, Public } from 'src/auth/auth.guard';
import {
  ApiOkResponsePaginated,
  PaginatedResponseDto,
} from 'src/shared/dto/paginated';
import { MessageResponseDto } from 'src/shared/dto/message';
import { CreateInterestDto, InterestSignupDto } from './dto/interest.dto';
import { InterestService } from './interest.service';

/**
 * The list of people waiting on the project. Nothing is sent from here:
 * there is no mail path out of this table yet, and the only reader is
 * the instance admin.
 */
@ApiTags('interest')
@Controller('interest')
@UseGuards(AuthGuard)
export class InterestController {
  constructor(private readonly interest: InterestService) {}

  /**
   * Ask to be kept posted. Public, because the landing page is where
   * this is asked and nobody has an account yet.
   *
   * The reply is the same whether the address was new or already on the
   * list, so the endpoint cannot be used to test whether someone signed
   * up.
   */
  @Post()
  @Public()
  async keepMePosted(
    @Body() request: CreateInterestDto,
  ): Promise<MessageResponseDto> {
    await this.interest.record(request.email, request.source || 'landing');
    return new MessageResponseDto('You are on the list.');
  }

  /**
   * Read the list.
   */
  @Get()
  @Admin()
  @ApiBearerAuth()
  @ApiOkResponsePaginated(InterestSignupDto)
  async listInterest(
    @Query('page') page: number,
  ): Promise<PaginatedResponseDto<InterestSignupDto>> {
    return this.interest.list(page || 1, 25);
  }
}

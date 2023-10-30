import { EntityManager } from '@mikro-orm/core';
import { Injectable } from '@nestjs/common';
import { RegistrationCodes } from 'src/entities/RegistrationCode';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';
import { RegistrationCodeDto } from './dto/registration.dto';

@Injectable()
export class RegistrationService {
  constructor(private readonly em: EntityManager) {}

  async createRegistrationCode(uses?: number): Promise<RegistrationCodeDto> {
    const registrationCode = new RegistrationCodes({
      uses: uses || 1,
    });

    await this.em.persistAndFlush(registrationCode);

    return new RegistrationCodeDto(registrationCode);
  }

  async listRegistrationCodes(
    page: number,
    pageSize: number,
  ): Promise<PaginatedResponseDto<RegistrationCodeDto>> {
    const [codes, count] = await this.em.findAndCount(
      RegistrationCodes,
      {},
      {
        offset: (page - 1) * pageSize,
        limit: pageSize,
      },
    );

    return PaginatedResponseDto.fromArray(
      page,
      Math.ceil(count / pageSize),
      codes.map((code) => new RegistrationCodeDto(code)),
    );
  }

  async deleteRegistrationCode(code: string) {
    const registrationCode = await this.em.findOneOrFail(RegistrationCodes, {
      id: code,
    });

    await this.em.removeAndFlush(registrationCode);
  }
}

import {
  EntityManager,
  UniqueConstraintViolationException,
} from '@mikro-orm/core';
import { Injectable } from '@nestjs/common';
import { InterestSignup } from 'src/entities/InterestSignup';
import { PaginatedResponseDto } from 'src/shared/dto/paginated';
import { InterestSignupDto } from './dto/interest.dto';

@Injectable()
export class InterestService {
  constructor(private readonly em: EntityManager) {}

  /**
   * Records an address, or leaves the existing row alone if it is
   * already there.
   *
   * Two addresses can race to the same insert, so the unique index is
   * what decides and the violation is the second one's answer rather
   * than an error: asking twice is not a mistake worth reporting.
   *
   * @param email Address to record; stored lowercased.
   * @param source Surface it came from.
   */
  async record(email: string, source: string): Promise<void> {
    const signup = new InterestSignup({
      email: email.trim().toLowerCase(),
      source,
    });

    try {
      await this.em.persistAndFlush(signup);
    } catch (error) {
      if (error instanceof UniqueConstraintViolationException) {
        this.em.clear();
        return;
      }
      throw error;
    }
  }

  async list(
    page: number,
    pageSize: number,
  ): Promise<PaginatedResponseDto<InterestSignupDto>> {
    const [signups, count] = await this.em.findAndCount(
      InterestSignup,
      {},
      {
        limit: pageSize,
        offset: (page - 1) * pageSize,
        orderBy: { createdAt: 'DESC' },
      },
    );

    return PaginatedResponseDto.fromArray(
      page,
      Math.ceil(count / pageSize),
      signups.map((signup) => new InterestSignupDto(signup)),
    );
  }
}

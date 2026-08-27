import { Controller, Get, ServiceUnavailableException } from '@nestjs/common';
import { EntityManager } from '@mikro-orm/postgresql';
import { ApiOperation, ApiTags } from '@nestjs/swagger';

/**
 * The probe target: readiness means the database answers, so a pod
 * that lost its connection stops receiving traffic instead of erroring
 * it. No auth on purpose; health is not a capability.
 */
@ApiTags('health')
@Controller('health')
export class HealthController {
  constructor(private readonly em: EntityManager) {}

  @Get()
  @ApiOperation({ summary: 'Service and database liveness.' })
  async health(): Promise<{ status: string }> {
    try {
      await this.em.getConnection().execute('select 1');
    } catch {
      throw new ServiceUnavailableException('Database unreachable.');
    }
    return { status: 'ok' };
  }
}

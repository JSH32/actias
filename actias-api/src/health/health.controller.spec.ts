import { HealthController } from './health.controller';
import { EntityManager } from '@mikro-orm/postgresql';
import { ServiceUnavailableException } from '@nestjs/common';

describe('HealthController', () => {
  const withConnection = (execute: jest.Mock) =>
    new HealthController({
      getConnection: () => ({ execute }),
    } as unknown as EntityManager);

  it('answers ok while the database does', async () => {
    const controller = withConnection(
      jest.fn().mockResolvedValue([{ '?column?': 1 }]),
    );
    await expect(controller.health()).resolves.toEqual({ status: 'ok' });
  });

  it('turns a dead database into 503, not 500', async () => {
    const controller = withConnection(
      jest.fn().mockRejectedValue(new Error('gone')),
    );
    await expect(controller.health()).rejects.toBeInstanceOf(
      ServiceUnavailableException,
    );
  });
});

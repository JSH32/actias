import { of } from 'rxjs';
import { QueuesController } from './queues.controller';
import { ResourcesService } from './resources.service';

const PROJECT = { id: 'project-1' } as any;

/** A controller whose data plane answers what each test injects. */
function controller(answers: { read?: unknown; dispatch?: unknown } = {}) {
  const readStats = jest.fn(() =>
    of({ valueJson: JSON.stringify(answers.read ?? null) }),
  );
  const dispatch = jest.fn(() =>
    of({ resultJson: JSON.stringify(answers.dispatch ?? null), error: '' }),
  );

  const grpc = (service: object) => ({ getService: () => service } as any);
  const resources = new ResourcesService(
    grpc({}),
    grpc({}),
    grpc({ readStats, dispatch }),
    { get: jest.fn(() => 'internal-token') } as any,
  );
  resources.onModuleInit();

  return { instance: new QueuesController(resources), readStats, dispatch };
}

describe('queue stats', () => {
  it('maps the worker read onto the dto', async () => {
    const { instance, readStats } = controller({
      read: { depth: 3, in_flight: 1, oldest_pending: 12, dead_letters: 2 },
    });

    const stats = await instance.queueStats(PROJECT, 'jobs');

    expect(stats).toEqual({
      depth: 3,
      inFlight: 1,
      oldestPending: 12,
      deadLetters: 2,
    });
    expect(readStats).toHaveBeenCalledWith(
      expect.objectContaining({
        scopeId: 'project-1',
        class: '__queue',
        name: 'jobs',
        firstHop: true,
      }),
      expect.anything(),
    );
  });

  it('reads a queue nothing has touched as zeros, not an error', async () => {
    const { instance } = controller({ read: null });

    const stats = await instance.queueStats(PROJECT, 'ghost');

    expect(stats).toEqual({
      depth: 0,
      inFlight: 0,
      oldestPending: undefined,
      deadLetters: 0,
    });
  });
});

describe('queue controls', () => {
  it('reports how many dead letters a retry requeued', async () => {
    const { instance, dispatch } = controller({ dispatch: 4 });

    const retried = await instance.retryDead(PROJECT, 'jobs');

    expect(retried).toEqual({ requeued: 4 });
    expect(dispatch).toHaveBeenCalledWith(
      expect.objectContaining({ method: 'retry_dead', class: '__queue' }),
      expect.anything(),
    );
  });
});

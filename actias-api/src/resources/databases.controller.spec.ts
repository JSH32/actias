import { of } from 'rxjs';
import { DatabasesController } from './databases.controller';
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

  return {
    instance: new DatabasesController(resources),
    readStats,
    dispatch,
  };
}

describe('the database overview', () => {
  it('maps the worker shapes onto the dto, camel-cased', async () => {
    const { instance } = controller({
      read: {
        size_bytes: 12288,
        tables: [
          {
            name: 'orders',
            rows: 7,
            columns: [
              {
                name: 'id',
                type: 'INTEGER',
                not_null: true,
                primary_key: true,
              },
            ],
          },
        ],
      },
    });

    const overview = await instance.databaseOverview(PROJECT, 'shop');

    expect(overview).toEqual({
      sizeBytes: 12288,
      tables: [
        {
          name: 'orders',
          rows: 7,
          columns: [
            { name: 'id', type: 'INTEGER', notNull: true, primaryKey: true },
          ],
        },
      ],
    });
  });

  it('reads a database nothing has shipped as empty, not an error', async () => {
    const { instance } = controller({ read: null });

    const overview = await instance.databaseOverview(PROJECT, 'ghost');

    expect(overview).toEqual({ sizeBytes: 0, tables: [] });
  });
});

describe('the sql console', () => {
  it('reads through the read bypass and writes through the owner', async () => {
    const { instance, dispatch } = controller({ dispatch: [{ n: 1 }] });

    const rows = await instance.query(PROJECT, 'shop', { sql: 'SELECT 1' });
    expect(rows).toEqual({ rows: [{ n: 1 }] });
    expect(dispatch).toHaveBeenLastCalledWith(
      expect.objectContaining({ method: 'read' }),
      expect.anything(),
    );

    await instance.execute(PROJECT, 'shop', { sql: 'DELETE FROM x' });
    expect(dispatch).toHaveBeenLastCalledWith(
      expect.objectContaining({ method: 'query' }),
      expect.anything(),
    );
  });
});

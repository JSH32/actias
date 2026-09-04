import { BadRequestException } from '@nestjs/common';
import { of } from 'rxjs';
import { ObjectsController } from './objects.controller';
import { ResourcesService, clampPageSize } from './resources.service';

const PROJECT = { id: 'project-1' } as any;

/** A controller over recording mocks; only the directory paths matter. */
function controller() {
  const listInstances = jest.fn(() => of({ instances: [], total: 0 }));
  const countInstances = jest.fn(() =>
    of({
      counts: [
        { class: 'UserCart', count: 10000 },
        { class: '__queue', count: 2 },
      ],
    }),
  );
  const listScripts = jest.fn(() => of({ scripts: [] }));

  const grpc = (service: object) => ({ getService: () => service } as any);
  const resources = new ResourcesService(
    grpc({ listScripts }),
    grpc({ listInstances, countInstances }),
    grpc({}),
    { get: jest.fn(() => 'internal-token') } as any,
  );
  resources.onModuleInit();

  return {
    instance: new ObjectsController(resources),
    listInstances,
    countInstances,
  };
}

describe('the directory page cap', () => {
  it('clamps unreasonable page sizes instead of erroring', () => {
    expect(clampPageSize(undefined)).toBe(100);
    expect(clampPageSize(0)).toBe(100);
    expect(clampPageSize(-5)).toBe(100);
    expect(clampPageSize(Number.NaN)).toBe(100);
    expect(clampPageSize(25)).toBe(25);
    expect(clampPageSize(99999)).toBe(500);
  });

  it('holds against a caller asking for everything at once', async () => {
    const { instance, listInstances } = controller();

    await instance.listObjects(PROJECT, 'UserCart', 'user-', '2', '99999');

    expect(listInstances).toHaveBeenCalledWith({
      projectIds: ['project-1'],
      class: 'UserCart',
      namePrefix: 'user-',
      pageSize: 500,
      page: 2,
    });
  });

  it('defaults to a bounded first page of the class', async () => {
    const { instance, listInstances } = controller();

    await instance.listObjects(PROJECT, 'UserCart');

    expect(listInstances).toHaveBeenCalledWith({
      projectIds: ['project-1'],
      class: 'UserCart',
      namePrefix: '',
      pageSize: 100,
      page: 0,
    });
  });

  it('refuses a listing that names no class', async () => {
    const { instance, listInstances } = controller();

    await expect(instance.listObjects(PROJECT)).rejects.toThrow(
      BadRequestException,
    );
    expect(listInstances).not.toHaveBeenCalled();
  });
});

describe('the class counts', () => {
  it('hides platform classes from the rail', async () => {
    const { instance } = controller();

    const counts = await instance.countObjects(PROJECT);

    // No contract declares a directory or a method in this fixture, so
    // the class carries the empty shape a console types against.
    expect(counts).toEqual([
      {
        class: 'UserCart',
        count: 10000,
        hasDirectory: false,
        directoryFields: [],
        methods: [],
      },
    ]);
  });
});

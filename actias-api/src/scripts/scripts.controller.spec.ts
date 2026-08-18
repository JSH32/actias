import { ForbiddenException } from '@nestjs/common';
import { of } from 'rxjs';
import { ScriptsController } from './scripts.controller';

const SCRIPT_ID = 'script-1';
const REVISION_ID = 'revision-1';
const PRINCIPAL = { user: { id: 'user-1' } } as any;

/** A controller whose service derives the given contract at publish. */
function controller(options: { derivedKv: string[]; aclAllows: boolean }) {
  const deleteRevision = jest.fn((request: unknown) => of({ request }));
  const createRevision = jest.fn((request: unknown) =>
    of({
      id: REVISION_ID,
      created: new Date().toISOString(),
      scriptId: SCRIPT_ID,
      scriptConfig: {
        id: SCRIPT_ID,
        entryPoint: 'main.lua',
        includes: [],
        ignore: [],
        capabilities: { kv: options.derivedKv, events: [], secrets: [] },
      },
      request,
    }),
  );
  const queryScript = jest.fn((request: unknown) =>
    of({ id: SCRIPT_ID, projectId: 'project-1', request }),
  );

  const subject = new ScriptsController(
    {
      getService: () => ({ createRevision, deleteRevision, queryScript }),
    } as any,
    {} as any,
    { findOneOrFail: jest.fn(async () => ({ id: 'project-1' })) } as any,
    {
      getPrincipalAccess: jest.fn(async () => ({
        test: () => options.aclAllows,
      })),
    } as any,
  );
  subject.onModuleInit();

  return { subject, deleteRevision };
}

/** A publish body; the bundle only needs to convert. */
function request() {
  return {
    bundle: { toServiceBundle: () => ({ entryPoint: 'main.lua', files: [] }) },
    scriptConfig: { id: SCRIPT_ID, entryPoint: 'main.lua' },
  } as any;
}

describe('ScriptsController.createRevision', () => {
  it('keeps a revision whose publisher holds kv access', async () => {
    const { subject, deleteRevision } = controller({
      derivedKv: ['visits'],
      aclAllows: true,
    });

    const revision = await subject.createRevision(
      SCRIPT_ID,
      request(),
      PRINCIPAL,
    );

    expect(revision.id).toBe(REVISION_ID);
    expect(deleteRevision).not.toHaveBeenCalled();
  });

  it('rolls back a kv-declaring revision when the publisher lacks kv access', async () => {
    // The contract is derived server-side, so only the response reveals
    // that the code declares kv; enforcement deletes what it refuses.
    const { subject, deleteRevision } = controller({
      derivedKv: ['visits'],
      aclAllows: false,
    });

    await expect(
      subject.createRevision(SCRIPT_ID, request(), PRINCIPAL),
    ).rejects.toBeInstanceOf(ForbiddenException);

    expect(deleteRevision).toHaveBeenCalledWith({ revisionId: REVISION_ID });
  });

  it('never consults the acl for a script declaring no kv', async () => {
    const { subject, deleteRevision } = controller({
      derivedKv: [],
      aclAllows: false,
    });

    const revision = await subject.createRevision(
      SCRIPT_ID,
      request(),
      PRINCIPAL,
    );

    expect(revision.id).toBe(REVISION_ID);
    expect(deleteRevision).not.toHaveBeenCalled();
  });
});

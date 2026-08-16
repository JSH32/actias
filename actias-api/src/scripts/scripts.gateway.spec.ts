import { WsException } from '@nestjs/websockets';
import { of } from 'rxjs';
import { ScriptsGateway } from './scripts.gateway';

const SCRIPT_ID = 'script-1';
const SESSION_ID = 'session-1';

/** Enough of a WebSocket for the gateway to talk to. */
function socket() {
  return { send: jest.fn(), close: jest.fn() } as any;
}

/**
 * Builds an update payload the gateway will accept, with overrides for the
 * fields a caller wants to make wrong.
 */
function update(overrides: { sessionId?: string; scriptId?: string } = {}) {
  return {
    sessionId: overrides.sessionId ?? SESSION_ID,
    scriptId: SCRIPT_ID,
    revision: {
      scriptConfig: { id: overrides.scriptId ?? SCRIPT_ID },
      bundle: {
        toServiceBundle: () => ({ entryPoint: 'main.lua', files: [] }),
      },
    },
  } as any;
}

/** A gateway whose script service records what it was asked to store. */
function gateway() {
  const putLiveSession = jest.fn((request: unknown) =>
    of({ sessionId: SESSION_ID, scriptId: SCRIPT_ID, request }),
  );

  const subject = new ScriptsGateway({
    getService: () => ({ putLiveSession }),
  } as any);
  subject.onModuleInit();

  return { subject, putLiveSession };
}

/** Opens a session so `handleUpdate` has something to validate against. */
async function connected() {
  const { subject, putLiveSession } = gateway();
  const client = socket();

  await subject.handleConnection(
    client,
    {} as any,
    {
      scriptId: SCRIPT_ID,
      revision: {
        scriptConfig: { id: SCRIPT_ID },
        bundle: {
          toServiceBundle: () => ({ entryPoint: 'main.lua', files: [] }),
        },
      },
    } as any,
  );

  putLiveSession.mockClear();
  return { subject, client, putLiveSession };
}

describe('ScriptsGateway', () => {
  it('accepts an update matching the open session', async () => {
    const { subject, client, putLiveSession } = await connected();

    await subject.handleUpdate(client, update());

    expect(putLiveSession).toHaveBeenCalledTimes(1);
    // The session id goes back out, which is what keeps an update replacing
    // the session the worker is serving instead of creating another.
    expect(putLiveSession.mock.calls[0][0]).toMatchObject({
      sessionId: SESSION_ID,
      scriptId: SCRIPT_ID,
    });
  });

  it('rejects an update naming a different session', async () => {
    const { subject, client, putLiveSession } = await connected();

    await expect(
      subject.handleUpdate(client, update({ sessionId: 'someone-elses' })),
    ).rejects.toBeInstanceOf(WsException);
    expect(putLiveSession).not.toHaveBeenCalled();
  });

  it('rejects an update naming a different script', async () => {
    const { subject, client, putLiveSession } = await connected();

    await expect(
      subject.handleUpdate(client, update({ scriptId: 'someone-elses' })),
    ).rejects.toBeInstanceOf(WsException);
    expect(putLiveSession).not.toHaveBeenCalled();
  });

  it('rejects an update from a socket that never connected', async () => {
    const { subject } = gateway();

    await expect(
      subject.handleUpdate(socket(), update()),
    ).rejects.toBeInstanceOf(WsException);
  });

  it('refuses a connection that already carries a session id', async () => {
    const { subject, putLiveSession } = gateway();
    const client = socket();

    await subject.handleConnection(
      client,
      {} as any,
      {
        sessionId: SESSION_ID,
        scriptId: SCRIPT_ID,
      } as any,
    );

    expect(client.close).toHaveBeenCalledWith(4001, expect.any(String));
    expect(putLiveSession).not.toHaveBeenCalled();
  });
});

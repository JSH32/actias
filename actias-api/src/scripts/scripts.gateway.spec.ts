import { WsException } from '@nestjs/websockets';
import { Subject, of } from 'rxjs';
import { ScriptsGateway } from './scripts.gateway';

const SCRIPT_ID = 'script-1';
const SESSION_ID = 'session-1';
const TOKEN = 'a-valid-token';
const USER = { id: 'user-1' } as any;

/** Enough of a WebSocket for the gateway to talk to. */
function socket() {
  return { send: jest.fn(), close: jest.fn() } as any;
}

/** The http upgrade request a browser or CLI would send. */
function upgrade(auth?: string) {
  return { headers: auth ? { authorization: auth } : {} } as any;
}

/** A live payload, with overrides for the fields a caller makes wrong. */
function payload(overrides: { sessionId?: string; scriptId?: string } = {}) {
  return {
    sessionId: overrides.sessionId,
    scriptId: SCRIPT_ID,
    revision: {
      scriptConfig: { id: overrides.scriptId ?? SCRIPT_ID },
      bundle: { entryPoint: 'main.lua', files: [] },
    },
  } as any;
}

/** A gateway over mocks that record what was stored and allow acl. */
function gateway(options: { aclAllows?: boolean } = {}) {
  const putLiveSession = jest.fn((request: unknown) =>
    of({ sessionId: SESSION_ID, scriptId: SCRIPT_ID, request }),
  );
  const queryScript = jest.fn((request: unknown) =>
    of({ id: SCRIPT_ID, projectId: 'project-1', request }),
  );
  // Live streams that emit what the test pushes into them.
  const logLines = new Subject<any>();
  const streamLiveLogs = jest.fn(() => logLines.asObservable());
  const streamScriptLogs = jest.fn(() => logLines.asObservable());

  const subject = new ScriptsGateway(
    {
      getService: () => ({
        putLiveSession,
        queryScript,
        streamLiveLogs,
        streamScriptLogs,
      }),
    } as any,
    {
      getUserFromToken: jest.fn(async (token: string) => {
        if (token !== TOKEN) throw new Error('bad token');
        return USER;
      }),
    } as any,
    {
      getProjectAccess: jest.fn(async () => ({
        test: () => options.aclAllows ?? true,
      })),
    } as any,
    {
      // Enough of an EntityManager for RequestContext.createAsync to wrap.
      name: 'default',
      fork: () => ({}),
      findOneOrFail: jest.fn(async () => ({ id: 'project-1' })),
    } as any,
  );
  subject.onModuleInit();

  return { subject, putLiveSession, logLines };
}

/** Authenticates a socket and starts a session on it. */
async function started(options: { aclAllows?: boolean } = {}) {
  const { subject, putLiveSession } = gateway(options);
  const client = socket();

  await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));
  await subject.handleStart(client, payload());

  putLiveSession.mockClear();
  return { subject, client, putLiveSession };
}

describe('ScriptsGateway', () => {
  it('closes a connection with no usable bearer token', async () => {
    const { subject } = gateway();

    const missing = socket();
    await subject.handleConnection(missing, upgrade());
    expect(missing.close).toHaveBeenCalledWith(4401, expect.any(String));

    const wrong = socket();
    await subject.handleConnection(wrong, upgrade('Bearer nonsense'));
    expect(wrong.close).toHaveBeenCalledWith(4401, expect.any(String));
  });

  it('confirms authentication with ready before anything else', async () => {
    const { subject } = gateway();
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));

    // Clients wait for this: messages sent before the async connection
    // handling finished would be dropped, so ready is the green light.
    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({ status: 'ready' }),
    );
  });

  it('starts a session and answers with its id', async () => {
    const { subject, putLiveSession } = gateway();
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));
    await subject.handleStart(client, payload());

    expect(putLiveSession).toHaveBeenCalledTimes(1);
    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({ status: 'created', sessionId: SESSION_ID }),
    );
  });

  it('refuses to start a session without project access', async () => {
    const { subject, putLiveSession } = gateway({ aclAllows: false });
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));

    await expect(subject.handleStart(client, payload())).rejects.toBeInstanceOf(
      WsException,
    );
    expect(putLiveSession).not.toHaveBeenCalled();
  });

  it('refuses messages from a socket that never authenticated', async () => {
    const { subject } = gateway();

    await expect(
      subject.handleStart(socket(), payload()),
    ).rejects.toBeInstanceOf(WsException);
  });

  it('accepts an update matching the open session', async () => {
    const { subject, client, putLiveSession } = await started();

    await subject.handleUpdate(client, payload({ sessionId: SESSION_ID }));

    expect(putLiveSession).toHaveBeenCalledTimes(1);
    // The session id goes back out, which is what keeps an update replacing
    // the session the worker is serving instead of creating another.
    expect(putLiveSession.mock.calls[0][0]).toMatchObject({
      sessionId: SESSION_ID,
      scriptId: SCRIPT_ID,
    });
  });

  it('rejects an update naming a different session', async () => {
    const { subject, client, putLiveSession } = await started();

    await expect(
      subject.handleUpdate(client, payload({ sessionId: 'someone-elses' })),
    ).rejects.toBeInstanceOf(WsException);
    expect(putLiveSession).not.toHaveBeenCalled();
  });

  it('rejects an update naming a different script', async () => {
    const { subject, client, putLiveSession } = await started();

    await expect(
      subject.handleUpdate(
        client,
        payload({ sessionId: SESSION_ID, scriptId: 'someone-elses' }),
      ),
    ).rejects.toBeInstanceOf(WsException);
    expect(putLiveSession).not.toHaveBeenCalled();
  });

  it('rejects an update before a session started', async () => {
    const { subject } = gateway();
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));

    await expect(
      subject.handleUpdate(client, payload({ sessionId: SESSION_ID })),
    ).rejects.toBeInstanceOf(WsException);
  });

  it('forwards the session log stream over the socket', async () => {
    const { subject, putLiveSession, logLines } = gateway();
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));
    await subject.handleStart(client, payload());
    putLiveSession.mockClear();
    client.send.mockClear();

    logLines.next({ level: 'info', message: 'hello logs', timestampMs: 5 });

    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({
        status: 'log',
        level: 'info',
        message: 'hello logs',
        timestampMs: 5,
      }),
    );
  });

  it('tails a script and forwards its production log stream', async () => {
    const { subject, logLines } = gateway();
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));
    await subject.handleTail(client, { scriptId: SCRIPT_ID });

    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({ status: 'tailing' }),
    );

    client.send.mockClear();
    logLines.next({
      level: 'warn',
      message: 'from production',
      timestampMs: 9,
    });

    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({
        status: 'log',
        level: 'warn',
        message: 'from production',
        timestampMs: 9,
      }),
    );
  });

  it('refuses a tail without read access', async () => {
    const { subject } = gateway({ aclAllows: false });
    const client = socket();

    await subject.handleConnection(client, upgrade(`Bearer ${TOKEN}`));

    await expect(
      subject.handleTail(client, { scriptId: SCRIPT_ID }),
    ).rejects.toBeInstanceOf(WsException);
  });

  it('ping re-stores the last payload so the session ttl moves', async () => {
    const { subject, client, putLiveSession } = await started();

    await subject.handlePing(client);

    expect(putLiveSession).toHaveBeenCalledTimes(1);
    expect(putLiveSession.mock.calls[0][0]).toMatchObject({
      sessionId: SESSION_ID,
      scriptId: SCRIPT_ID,
    });
    expect(client.send).toHaveBeenCalledWith(
      JSON.stringify({ status: 'alive' }),
    );
  });
});

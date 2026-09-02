-- Queues and workflows.
--
-- A workflow body runs like ordinary code but replays from a journal
-- after crashes and deploys, so effects belong inside steps and
-- everything else must be deterministic. The long form is the
-- workflows docs page; the shapes here are the api.

-- A duration is written "500ms", "30s", "10m", "24h" or "7d", or as a
-- bare number of seconds. One spelling everywhere: alarms, timeouts,
-- sleeps and `expire` all parse it identically.
type Duration = string | number

type Queue = {
    -- Enqueue and move on; the payload is json-serializable. Answers
    -- true once the message is durable; a queue that cannot store it
    -- raises.
    send: (self: Queue, payload: any) -> boolean,
    stats: (self: Queue) -> any,
}

-- One run of a workflow, reached from start/get inside the app or via
-- `workflows` from outside it.
type WorkflowRun = {
    -- Deliver a named signal the body is (or will be) waiting on.
    -- Duplicates are safe; an unconsumed extra is set aside for the
    -- next wait on that name.
    signal: (self: WorkflowRun, name: string, payload: any?) -> any,
    cancel: (self: WorkflowRun, reason: string?) -> any,
    status: (self: WorkflowRun) -> any,
    started: any,
}

type WorkflowDefinition = {
    -- Begin a run under an id you choose; starting an id that is
    -- already running is a no-op, which is what makes retries safe.
    start: (self: WorkflowDefinition, input: any, opts: { id: string }) -> WorkflowRun,
    get: (self: WorkflowDefinition, id: string) -> WorkflowRun,
}

-- A child run spawned from a body, waitable by all/race alongside
-- plain signal names.
type WfJob = {
    name: string,
    signal: string,
}

-- The first parameter of every workflow body. Each call on it is
-- journaled: a step body executes once and its return value replays
-- from the journal afterwards, waits resume where they stopped.
type Wf = {
    step_id: string,
    -- step(name, body) or step(name, { retries = n }, body). The body
    -- holds the run's effects; its return value is what replays, so
    -- read clocks and randomness inside one. The same name in a loop
    -- journals once per pass.
    step: (self: Wf, name: string, body: (() -> any) | { any }, body2: (() -> any)?) -> any,
    sleep: (self: Wf, duration: Duration) -> (),
    -- The signal payload, or nil when the timeout won.
    await: (self: Wf, name: string, opts: { timeout: Duration? }?) -> any,
    -- Start another workflow as a child job for all/race.
    spawn: (self: Wf, definition: string, input: any) -> WfJob,
    -- Payloads in argument order; a timed-out entry is nil.
    all: (self: Wf, jobs: { WfJob | string }, opts: { timeout: Duration? }?) -> { any },
    -- The first payload to arrive and the name it arrived under.
    race: (self: Wf, jobs: { WfJob | string }, opts: { timeout: Duration? }?) -> (any, string?),
}

declare queue: (name: string) -> Queue
declare workflow: (name: string) -> ((Wf, any) -> any) -> WorkflowDefinition
-- The reference form, for reaching a run from code that does not hold
-- the definition (a route file signalling a workflow the domain owns).
declare workflows: (name: string) -> WorkflowDefinition

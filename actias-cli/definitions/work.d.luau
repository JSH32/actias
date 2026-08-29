-- Queues and workflows.

type Queue = {
    send: (self: Queue, payload: any) -> boolean,
    stats: (self: Queue) -> any,
}

type WorkflowRun = {
    signal: (self: WorkflowRun, name: string, payload: any?) -> any,
    cancel: (self: WorkflowRun, reason: string?) -> any,
    status: (self: WorkflowRun) -> any,
    started: any,
}

type WorkflowDefinition = {
    start: (self: WorkflowDefinition, input: any, opts: { id: string }) -> WorkflowRun,
    get: (self: WorkflowDefinition, id: string) -> WorkflowRun,
}

type WfJob = {
    name: string,
    signal: string,
}

type Wf = {
    step_id: string,
    step: (self: Wf, name: string, body: (() -> any) | { any }, body2: (() -> any)?) -> any,
    sleep: (self: Wf, duration: string | number) -> (),
    await: (self: Wf, name: string, opts: { timeout: (string | number)? }?) -> any,
    spawn: (self: Wf, definition: string, input: any) -> WfJob,
    all: (self: Wf, jobs: { WfJob | string }, opts: { timeout: (string | number)? }?) -> { any },
    race: (self: Wf, jobs: { WfJob | string }, opts: { timeout: (string | number)? }?) -> (any, string?),
}

declare queue: (name: string) -> Queue
declare workflow: (name: string) -> ((Wf, any) -> any) -> WorkflowDefinition
declare workflows: (name: string) -> WorkflowDefinition

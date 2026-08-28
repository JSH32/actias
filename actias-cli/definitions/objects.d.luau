-- Durable objects: classes, state, streams.

type Sql = {
    exec: (self: Sql, sql: string, params: { any }?) -> number,
    query: (self: Sql, sql: string, params: { any }?) -> { any },
    query_one: (self: Sql, sql: string, params: { any }?) -> any,
}

-- One delivered stream event, as receives handlers and sock loops see it.
type StreamEvent = { topic: string, from: { id: string, class: string, name: string }, data: any }

-- The identity a follow gate sees.
type Follower = {
    name: string,
    transport: string,
    id: string,
    is: (self: Follower, class: any) -> boolean,
}

-- The state table an object's hooks and methods receive. Plain keys
-- are in-memory scratch and die with the vm; `sql` is the instance's
-- own durable storage. The class's own routable methods ride the
-- indexer too: `state:method(...)` dispatches them directly (no
-- mailbox, so no self-call cycle), which is how sibling behavior is
-- reached from hooks and other methods.
type ObjectState = {
    name: string,
    sql: Sql,
    now: () -> number,
    set_alarm: (self: ObjectState, duration: string | number) -> (),
    -- Forget this object once the current call commits and answers:
    -- storage, snapshot and edges are reclaimed, queued calls refuse,
    -- and naming the identity again builds a fresh instance.
    destroy: (self: ObjectState) -> (),
    publish: (self: ObjectState, topic: string, event: any) -> (),
    follow: (self: ObjectState, target: any, topic: string, filter: any?) -> boolean,
    unfollow: (self: ObjectState, target: any, topic: string) -> boolean,
    followers: (self: ObjectState, topic: string?) -> { any },
    drop_followers: (self: ObjectState, match: any) -> (),
    [string]: any,
}

-- A class body: `publishes` entries are bare topics (the hooks.follow
-- gate decides) or topic = policy ("self", "public"); `receives` maps
-- "Class:topic" to a handler; `migrations` names a bundle dir applied
-- to each instance's own storage before init (init only ever seeds);
-- every other key is a routable method.
type ClassBody = {
    publishes: { [number | string]: string }?,
    migrations: string?,
    -- A name this returns false for never comes to exist: no lease, no
    -- row, no file. Runs once per fresh identity, never on access.
    admit: ((name: string) -> boolean)?,
    -- An instance untouched this long is deleted by the platform; touch
    -- refreshes. A pending alarm blocks expiry.
    expire: string?,
    receives: { [string]: (state: ObjectState, event: StreamEvent) -> () }?,
    hooks: {
        init: ((state: ObjectState) -> ())?,
        alarm: ((state: ObjectState) -> ())?,
        follow: ((state: ObjectState, topic: string, follower: Follower) -> boolean)?,
    }?,
    -- Methods are state-first; the new solver types their parameter
    -- from this indexer, so `state` needs no annotation in a method.
    [string]: (state: ObjectState, ...any) -> any,
}

-- Callable: `Room("lobby")` is the blessed spelling of `Room:get("lobby")`.
-- Generic over the class body, so an instance offers the class's own
-- methods: their state-first signatures are what `:` calls satisfy.
-- The reference form has no body to carry, so its instances stay any.
type ObjectHandle<Body> = ((name: string) -> Body) & {
    get: (self: ObjectHandle<Body>, name: string) -> Body,
}

declare object: (class: string) -> <Body>(body: Body & ClassBody) -> ObjectHandle<Body>
declare objects: (class: string) -> ObjectHandle<any>

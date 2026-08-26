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
-- own durable storage.
type ObjectState = {
    name: string,
    sql: Sql,
    now: () -> number,
    set_alarm: (self: ObjectState, duration: string | number) -> (),
    publish: (self: ObjectState, topic: string, event: any) -> (),
    follow: (self: ObjectState, target: any, topic: string, filter: any?) -> boolean,
    unfollow: (self: ObjectState, target: any, topic: string) -> boolean,
    followers: (self: ObjectState, topic: string?) -> { any },
    drop_followers: (self: ObjectState, match: any) -> (),
    [string]: any,
}

-- A class body: `publishes` entries are bare topics (the hooks.follow
-- gate decides) or topic = policy ("self", "public"); `receives` maps
-- "Class:topic" to a handler; every other key is a routable method.
type ClassBody = {
    publishes: { [number | string]: string }?,
    receives: { [string]: (state: ObjectState, event: StreamEvent) -> () }?,
    hooks: {
        init: ((state: ObjectState) -> ())?,
        alarm: ((state: ObjectState) -> ())?,
        follow: ((state: ObjectState, topic: string, follower: Follower) -> boolean)?,
    }?,
    [string]: any,
}

-- Callable: `Room("lobby")` is the blessed spelling of `Room:get("lobby")`.
-- Generic over the class body, so an instance offers the class's own
-- methods: their state-first signatures are what `:` calls satisfy.
-- The reference form has no body to carry, so its instances stay any.
type ObjectHandle<Body> = ((name: string) -> Body) & {
    get: (self: ObjectHandle<Body>, name: string) -> Body,
}

declare object: (class: string) -> <Body>(body: Body) -> ObjectHandle<Body>
declare objects: (class: string) -> ObjectHandle<any>

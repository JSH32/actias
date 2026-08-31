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
-- One page of the store, ascending by key; a page without a cursor is
-- the last, and the cursor feeds the next list call.
type StorePage = {
    entries: { { key: string, value: any } },
    cursor: string?,
}

-- The key-value face over the object's own storage: same file and same
-- transaction as sql, no migrations needed. Values are json-serializable
-- and capped at 64 KB each; past that, the state wants a table.
type Store = {
    get: (self: Store, key: string) -> any,
    set: (self: Store, key: string, value: any) -> (),
    delete: (self: Store, key: string) -> (),
    -- A table of key -> value; a nil value deletes.
    set_batch: (self: Store, values: { [string]: any }) -> (),
    list: (
        self: Store,
        options: { prefix: string?, limit: number?, cursor: string? }?
    ) -> StorePage,
}

type ObjectState = {
    name: string,
    sql: Sql,
    store: Store,
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
}

-- What `object` accepts: a table whose every entry is a state-first
-- method. The reserved keys a body may also carry are `publishes`
-- (bare topics, or topic = "self" / "public"), `migrations` (a bundle
-- dir applied to each instance before init), `receives`
-- ("Class:topic" -> handler), `admit`, `expire` and `hooks`
-- (init, alarm, follow).
--
-- Those keys are deliberately NOT fields of this type. A generic
-- constraint carrying named optional fields makes Luau's inference fail
-- whenever a body omits one, which is what made `--!strict` unusable on
-- any class that did not happen to declare `migrations`. Their names and
-- shapes are enforced where they are actually read: RESERVED_METHODS in
-- actias-declarations, and the declaration pass behind `actias check`.
type ClassBody = {
    [string]: (state: ObjectState, ...any) -> any,
}

-- Sibling dispatch and scratch keys in a `--!strict` file: name the
-- class's own state type and annotate the methods with it. `ObjectState`
-- carries no catch-all, so this is also what makes `state:typo()` an
-- error instead of silently passing.
--
--     type AccountState = ObjectState & {
--         face: (AccountState) -> Face,
--         cached: Face?,
--     }
--     local Account = object "Account" {
--         face = function(state: AccountState) ... end,
--         home = function(state: AccountState) return state:face() end,
--     }
--
-- Without the annotation a method's state is `ObjectState`, which is
-- everything the platform gives you and nothing the class adds.

-- Callable: `Room("lobby")` is the blessed spelling of `Room:get("lobby")`.
-- Generic over the class body, so an instance offers the class's own
-- methods: their state-first signatures are what `:` calls satisfy.
-- The reference form has no body to carry, so its instances stay any.
type ObjectHandle<Body> = ((name: string) -> Body & ObjectState) & {
    get: (self: ObjectHandle<Body>, name: string) -> Body & ObjectState,
}

declare object: (class: string) -> <Body>(body: Body & ClassBody) -> ObjectHandle<Body>
declare objects: (class: string) -> ObjectHandle<any>

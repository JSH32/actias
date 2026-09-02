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
-- own durable storage. At runtime the class's own methods are reachable
-- through it too: `state:method(...)` dispatches them directly (no
-- mailbox, so no self-call cycle). The type deliberately does not carry
-- them; a class that wants them typed names its own state, see the
-- strict-mode note above `object`.

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
    follow: (self: ObjectState, target: any, topic: string, filter: any?) -> (),
    unfollow: (self: ObjectState, target: any, topic: string) -> (),
    followers: (self: ObjectState, topic: string?) -> { any },
    drop_followers: (self: ObjectState, match: any) -> (),
}

-- The state a `directory` function sees: reads only. The verbs that
-- change the object (set_alarm, destroy, publish, follow) are absent,
-- and sql refuses to write, because the row is derived rather than
-- decided: the platform re-derives it from a restored copy when it
-- backfills a new field or verifies a stale one, where there is no
-- instance to act on.
type DirectoryState = {
    name: string,
    sql: Sql,
    store: Store,
    now: () -> number,
}

-- What one directory field holds once derived. A nil is absent rather
-- than null, and an array holds scalars.
type DirectoryValue = string | number | boolean | { string | number | boolean }

-- The kind a declared field holds. Publish records these, so a query
-- knows a field is on every row rather than guessing from the rows it
-- happens to see, and the store binds each value as the kind declared
-- (a number compared as text answers "75" > "100").
export type DirectoryKind = "string" | "integer" | "number" | "boolean" | "array"

-- A kind marker from the `f` kit. Bare, it reads the field's own name
-- from what `from` returned; called with an extractor, it reads
-- whatever that extractor projects out of the same value. Calling one
-- answers a marker, so both spellings are the same kind of thing.
export type DirectoryField =
    (extract: (source: any, state: DirectoryState) -> DirectoryValue?) -> DirectoryField

-- The kinds a field may be declared with, one marker each.
type DirectoryFieldKit = {
    string: DirectoryField,
    integer: DirectoryField,
    number: DirectoryField,
    boolean: DirectoryField,
    array: DirectoryField,
}

-- A class's directory: what one shared read gets, and the fields it
-- feeds. Both are required. Fields with no `from` read nothing; a
-- `from` with no fields exposes nothing queryable.
--
-- Changing `fields` is what tells the platform a backfill is owed, so
-- an ordinary deploy that only changes VALUES costs nothing.
export type DirectoryDeclaration = {
    from: (state: DirectoryState) -> any,
    fields: { [string]: DirectoryField },
}

-- What `object` accepts: a table whose every entry is a state-first
-- method. The reserved keys a body may also carry are `publishes`
-- (bare topics, or topic = "self" / "public"), `migrations` (a bundle
-- dir applied to each instance before init), `receives`
-- ("Class:topic" -> handler), `admit`, `expire`, `directory`
-- (a DirectoryDeclaration: the queryable fields this class exposes and
-- where they come from) and `hooks` (init, alarm, follow).
--
-- The body's type is deliberately unconstrained. Named optional fields
-- in a generic constraint make inference fail whenever a body omits one,
-- and an indexer over methods puts that indexer on every instance, which
-- turns unknown-method calls and wrong return types into silent `any`.
-- Reserved keys are enforced where they are actually read instead:
-- RESERVED_METHODS in actias-declarations, and the declaration pass
-- behind `actias check`.

-- Strict mode: declare your class's state type and annotate each
-- method with it, the way a React component types its props in TS.
-- The checker cannot know your class's shape, so you name it once and
-- every method binds to it; in exchange, calling a method that does
-- not exist and misusing a return are both errors at the call site.
--
--     type AccountState = ObjectState & {
--         face: (AccountState) -> Face,   -- sibling dispatch
--         cached: Face?,                  -- scratch key
--     }
--     local Account = object "Account" {
--         face = function(state: AccountState) ... end,
--         home = function(state: AccountState) return state:face() end,
--     }
--
-- `state: ObjectState` alone is enough for a class with no scratch keys
-- that never calls its own siblings. A bare `function(state)` in a
-- strict file leaves state untyped (a free generic, like an untyped
-- props object), so its uses error until you annotate. Nonstrict files
-- can leave state bare; it is any there, as before.

-- Two classes that name each other need one of them forward-declared,
-- and in a `--!strict` file a bare `local Other` is `T?`, which cannot
-- be called at all. Reach it through a local function instead, which is
-- resolved when it runs rather than where it is written:
--
--     local Other
--     local function other(name: string) return assert(Other)(name) end
--     local First = object "First" {
--         m = function(state) return other("k"):n() end,
--     }
--     Other = object "Other" { n = function(state) return "x" end }

-- What one field may be compared against.
type DirectoryOperand = string | number | boolean

-- A filter on one field: a bare value is equality, a table is
-- operators. `one_of` rather than `in`, because `in` is a Luau keyword
-- and would not parse as a key.
type DirectoryFilter = DirectoryOperand | {
    eq: DirectoryOperand?,
    ne: DirectoryOperand?,
    lt: DirectoryOperand?,
    lte: DirectoryOperand?,
    gt: DirectoryOperand?,
    gte: DirectoryOperand?,
    one_of: { DirectoryOperand }?,
    starts_with: string?,
    -- Membership in an array field.
    contains: DirectoryOperand?,
    -- Presence. `exists = false` is the only way to match a field the
    -- object does not have; comparisons never match absence.
    exists: boolean?,
}

-- Field name to filter, plus the combinators. Top-level entries are
-- ANDed; `any`, `all` and `none` nest full where tables.
export type DirectoryWhere = { [string]: DirectoryFilter | { DirectoryWhere } }

-- One matching object: its name, plus whatever fields the class's
-- directory function exposes.
export type DirectoryEntry = { name: string, [string]: any }

export type DirectoryPage = {
    entries: { DirectoryEntry },
    -- Feeds the next call; absent on the last page.
    cursor: string?,
}

-- One entry of a verified read. `unverified` says the row was served
-- without proof (the object's manifest could not be read, or its last
-- derivation failed), with `reason` saying which. Such a row is served
-- rather than dropped: a listing may name an object that no longer
-- matches, and never fails to name one that does.
export type DirectoryVisitEntry = DirectoryEntry & {
    unverified: boolean,
    reason: string?,
}

export type DirectoryVisitPage = {
    entries: { DirectoryVisitEntry },
    -- Continues where this page's candidates ended. Verification drops
    -- rows, so a short page with a cursor is normal, not the end.
    cursor: string?,
}

-- The options a read takes.
type DirectoryOptions = {
    where: DirectoryWhere?,
    -- Field to "asc" or "desc"; `name` breaks every tie.
    order: { [string]: string }?,
    limit: number?,
    cursor: string?,
}

-- Callable: `Room("lobby")` is the blessed spelling of `Room:get("lobby")`.
-- Generic over the class body, so an instance offers the class's own
-- methods: their state-first signatures are what `:` calls satisfy.
--
-- The three reads query the class's directory: one row per object,
-- answered without waking any of them. They see each object as of its
-- last saved write, so they choose who to call; the object itself is
-- the truth.
--
-- `list` takes the option table, `find` takes the predicate alone with
-- default order and limit, and `visit` is `list` with every candidate
-- checked against its own object before it is served.
type ObjectHandle<Body> = ((name: string) -> Body & ObjectState) & {
    get: (self: ObjectHandle<Body>, name: string) -> Body & ObjectState,
    list: (self: ObjectHandle<Body>, options: DirectoryOptions?) -> DirectoryPage,
    find: (self: ObjectHandle<Body>, predicate: DirectoryWhere?) -> DirectoryPage,
    visit: (self: ObjectHandle<Body>, options: DirectoryOptions?) -> DirectoryVisitPage,
}

-- The reference form names a class whose body is not in scope, so its
-- instances are `any` and a call through one checks nothing. Spelled out
-- rather than reached as `ObjectHandle<any>`: that yields `any &
-- ObjectState`, which Luau keeps as an intersection rather than
-- collapsing, so every method call on it is a missing key.
type AnyHandle = ((name: string) -> any) & {
    get: (self: AnyHandle, name: string) -> any,
    -- The reads answer from the class's own index, so they work through
    -- a reference exactly as through the declaration: the rows belong
    -- to the project, not to the script that declared the class.
    list: (self: AnyHandle, options: DirectoryOptions?) -> DirectoryPage,
    find: (self: AnyHandle, predicate: DirectoryWhere?) -> DirectoryPage,
    visit: (self: AnyHandle, options: DirectoryOptions?) -> DirectoryVisitPage,
}

declare object: (class: string) -> <Body>(body: Body) -> ObjectHandle<Body>
declare objects: (class: string) -> AnyHandle
declare f: DirectoryFieldKit

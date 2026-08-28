-- Everyday globals: declarations, utilities, kv and sql storage.

-- One page of a namespace, ascending by key. A page without a cursor
-- is the last one; pass the cursor back to list for the next page.
type KvPage = {
    entries: { { key: string, value: any } },
    cursor: string?,
}

type KvNamespace = {
    get: (self: KvNamespace, key: string) -> any,
    set: (self: KvNamespace, key: string, value: any) -> (),
    -- A table of key -> value in one round trip; a nil value deletes.
    set_batch: (self: KvNamespace, values: { [string]: any }) -> (),
    delete: (self: KvNamespace, ...string) -> (),
    list: (self: KvNamespace, options: { limit: number?, cursor: string? }?) -> KvPage,
}

type Database = {
    exec: (self: Database, sql: string, params: { any }?) -> number,
    query: (self: Database, sql: string, params: { any }?) -> { any },
    query_one: (self: Database, sql: string, params: { any }?) -> any,
    read: (self: Database, sql: string, params: { any }?) -> { any },
    read_one: (self: Database, sql: string, params: { any }?) -> any,
    batch: (self: Database, statements: { any }) -> any,
}

declare kv: (namespace: string) -> KvNamespace
declare secret: (name: string) -> string
-- "fetch" is a known event with a typed request; queue:* and cron:*
-- carry runtime names, so they stay open strings.
declare on: ((event: "fetch") -> ((HttpRequest) -> any) -> ()) & ((event: string) -> ((any) -> any) -> ())
-- The handle also takes a declaration body (`database "catalog"
-- { migrations = "dir" }`) and returns itself; bare use is the same
-- handle without one.
type DatabaseHandle = ((body: { migrations: string? }) -> Database) & Database

declare database: (name: string) -> DatabaseHandle
declare json: { stringify: (any) -> string, parse: (string) -> any }
declare log: { debug: (...any) -> (), info: (...any) -> (), warn: (...any) -> (), error: (...any) -> () }
declare uuid: { v4: () -> string }
declare getfile: (path: string) -> { number }?
declare dofile: (path: string) -> any
declare require: (path: string) -> any
declare script: { identifier: string }

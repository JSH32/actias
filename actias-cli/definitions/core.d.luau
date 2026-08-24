-- Everyday globals: declarations, utilities, kv and sql storage.

type KvNamespace = {
    get: (self: KvNamespace, key: string) -> any,
    set: (self: KvNamespace, key: string, value: any) -> (),
    delete: (self: KvNamespace, key: string) -> (),
    list: (self: KvNamespace) -> { [string]: any },
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
declare on: (event: string) -> ((any) -> any) -> ()
declare database: (name: string) -> Database
declare json: { stringify: (any) -> string, parse: (string) -> any }
declare log: { debug: (any) -> (), info: (any) -> (), warn: (any) -> (), error: (any) -> () }
declare uuid: { v4: () -> string }
declare getfile: (path: string) -> { number }?
declare dofile: (path: string) -> any
declare require: (path: string) -> any
declare script: { identifier: string }

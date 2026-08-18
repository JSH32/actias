-- Ambient Luau declarations for the actias runtime surface; `actias check`
-- hands this to luau-analyze so scripts type-check against the platform.

declare class KvNamespace
    function get(self, key: string): any
    function set(self, key: string, value: any): ()
    function delete(self, key: string): ()
    function list(self): { [string]: any }
end

declare function kv(namespace: string): KvNamespace
declare function secret(name: string): string
declare function on(event: string): ((request: any) -> any) -> ()

declare json: {
    stringify: (value: any) -> string,
    parse: (raw: string) -> any,
}

declare log: {
    debug: (message: any) -> (),
    info: (message: any) -> (),
    warn: (message: any) -> (),
    error: (message: any) -> (),
}

declare uuid: {
    v4: () -> string,
}

declare function getfile(path: string): { number }?
declare function dofile(path: string): any

-- Loosely typed until their surfaces settle.
declare http: any
declare crypto: any
declare jwt: any
declare script: any

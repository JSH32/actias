-- Http in and out, websockets, and auth primitives.

type Uri = {
    scheme: string?,
    authority: any,
    path: string,
    query: string?,
    tostring: (self: Uri) -> string,
}

-- The conn surface a declared connection's handlers receive: the wire
-- verbs plus the identity it speaks as and its json-serializable
-- session scratchpad.
type Conn = {
    send: (self: Conn, data: any) -> boolean,
    follow: (self: Conn, target: any, topic: string, filter: any?) -> boolean,
    unfollow: (self: Conn, target: any, topic: string) -> boolean,
    close: (self: Conn) -> boolean,
    state: { [string]: any },
    name: string,
    class: string,
}

-- A declared connection program: handlers for the wire's stimuli.
-- Nothing calls a connection, so a body declares handlers, not methods.
type ConnectionBody = {
    open: ((conn: Conn) -> ())?,
    frame: ((conn: Conn, data: any) -> ())?,
    -- A function shapes each delivery into this app's own frame;
    -- "forward" sends it as { type = "event", topic, from, data }
    -- without building a vm. Declared as neither, deliveries drop.
    event: (((conn: Conn, event: StreamEvent) -> ()) | "forward")?,
    close: ((conn: Conn) -> ())?,
    -- The connection's own heartbeat, gone with the wire; `missed`
    -- counts ticks that fired while a previous run was still going.
    timer: { every: string, run: (conn: Conn, missed: number) -> () }?,
}
type ConnectionClass = { __connection: string }

declare connection: (class: string) -> (body: ConnectionBody) -> ConnectionClass

-- An incoming request. `upgrade` is typed present (not optional):
-- calling through an optional field leaves the old solver unable to
-- pick the function type; non-upgradable requests refuse at runtime
-- either way. The seed becomes the connection's initial conn.state
-- and sits between the class and the identity; two arguments means no
-- seed.
type HttpRequest = {
    uri: string,
    method: string?,
    headers: { [string]: string }?,
    body: any,
    version: string?,
    context_uri: string?,
    path: string?,
    query: { [string]: string }?,
    upgrade: (self: HttpRequest, program: ConnectionClass, seed_or_identity: any, identity: any?) -> any,
}

-- What make_request sends; outgoing requests carry no upgrade.
type HttpRequestInit = {
    uri: string,
    method: string?,
    headers: { [string]: string }?,
    body: any?,
}

type HttpResponse = {
    status_code: number?,
    headers: { [string]: string }?,
    body: any,
}

type HttpModule = {
    make_request: (request: HttpRequestInit) -> HttpResponse,
    Uri: { parse: (uri: string) -> Uri, new: (parts: any) -> Uri },
}

type JwtAlgorithm = "HS256" | "HS384" | "HS512"

type JwtSigner = {
    encode: (self: JwtSigner, claims: { [string]: any }) -> string,
    decode: (self: JwtSigner, token: string) -> { [string]: any }?,
}

type JwtStatic = {
    new: (algorithm: JwtAlgorithm, secret: string) -> JwtSigner,
}

type Argon2 = {
    hash: (self: Argon2, password: string) -> string,
    verify: (self: Argon2, hash: string, password: string) -> boolean,
}

type RsaPublicKey = {
    encrypt: (self: RsaPublicKey, data: { number }) -> { number },
}

type RsaPrivateKey = {
    to_pem: (self: RsaPrivateKey, kind: ("PKCS1" | "PKCS8")?) -> string,
    decrypt: (self: RsaPrivateKey, data: { number }) -> { number },
    public_key: (self: RsaPrivateKey) -> RsaPublicKey,
}

type CryptoModule = {
    sha224: (data: string) -> string,
    sha256: (data: string) -> string,
    sha384: (data: string) -> string,
    sha512: (data: string) -> string,
    sha512_224: (data: string) -> string,
    sha512_256: (data: string) -> string,
    Argon2: { new: (algorithm: "Argon2d" | "Argon2i" | "Argon2id") -> Argon2 },
    RsaPrivateKey: {
        new: (bits: number) -> RsaPrivateKey,
        from_pem: (pem: string, kind: ("PKCS1" | "PKCS8")?) -> RsaPrivateKey,
    },
}

declare http: HttpModule
declare crypto: CryptoModule
declare Jwt: JwtStatic
declare jwt: { Jwt: JwtStatic }

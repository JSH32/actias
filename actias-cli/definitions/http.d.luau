-- Http in and out, websockets, and auth primitives.

type Uri = {
    scheme: string?,
    authority: any,
    path: string,
    query: string?,
    tostring: (self: Uri) -> string,
}

-- The sock value connection programs receive from request:upgrade.
-- `each` takes a handler because Luau cannot resume a yield inside a
-- generic-for iterator; `recv` is the manual-while primitive.
type SocketItem = { kind: string, event: StreamEvent?, data: any }
type Socket = {
    each: (self: Socket, handler: (SocketItem) -> boolean?) -> (),
    recv: (self: Socket) -> SocketItem?,
    send: (self: Socket, data: any) -> boolean,
    follow: (self: Socket, target: any, topic: string, filter: any?) -> boolean,
    unfollow: (self: Socket, target: any, topic: string) -> boolean,
    close: (self: Socket) -> boolean,
}

type HttpRequest = {
    uri: string,
    method: string?,
    headers: { [string]: string }?,
    body: any,
    version: string?,
    context_uri: string?,
    path: string?,
    query: { [string]: string }?,
    upgrade: ((self: HttpRequest, program: (Socket) -> (), identity: any) -> any)?,
}

type HttpResponse = {
    status_code: number?,
    headers: { [string]: string }?,
    body: any,
}

type HttpModule = {
    make_request: (request: HttpRequest) -> HttpResponse,
    Uri: { parse: (uri: string) -> Uri, new: (parts: any) -> Uri },
}

type JwtAlgorithm =
    "HS256" | "HS384" | "HS512"
    | "RS256" | "RS384" | "RS512"
    | "PS256" | "PS384" | "PS512"
    | "ES256" | "ES384" | "EdDSA"

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

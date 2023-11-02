---@meta
---@diagnostic disable: lowercase-global, missing-return
---@class JwtHeader Table object that contains typ and alg fields.
---@field typ "JWT" Header type.
---@field alg JwtAlgorithm Algorithm type.
local JwtHeader = {}

---@alias JwtAlgorithm
---| '"HS256"' # HMAC using SHA-256 hash algorithm
---| '"HS384"' # HMAC using SHA-384 hash algorithm
---| '"HS512"' # HMAC using SHA-512 hash algorithm
---| '"RS256"' # RSASSA-PKCS1-v1_5 using SHA-256 hash algorithm
---| '"RS384"' # RSASSA-PKCS1-v1_5 using SHA-384 hash algorithm
---| '"RS512"' # RSASSA-PKCS1-v1_5 using SHA-512 hash algorithm
---| '"PS256"' # RSASSA-PSS using SHA-256 hash algorithm
---| '"PS384"' # RSASSA-PSS using SHA-384 hash algorithm
---| '"PS512"' # RSASSA-PSS using SHA-512 hash algorithm
---| '"ES256"' # ECDSA using P-256 curve and SHA-256 hash algorithm
---| '"ES384"' # ECDSA using P-384 curve and SHA-384 hash algorithm
---| '"EdDSA"' # ECDSA using P-521 curve and SHA-512 hash algorithm

---@class Jwt A JSON Web Token generator and verifier.
Jwt = {}

---Create a new Jwt signer object.
---@param header JwtHeader Token header.
---@param secret string Secret string to encode/decode the jwt.
---@return Jwt
function Jwt.new(header, secret) end

---Encode a table into a jwt string.
---@param table table table to encode into a jwt string.
---@return string # jwt string representation of the provided table.
function Jwt:encode(table) end

---Decode a jwt string into a table.
---@param token string jwt string to decode into a table.
---@return table | nil # table representation of the provided jwt string. If the JWT is invalid, nil is returned.
function Jwt:decode(token) end

---Exposed JWT module
jwt = {}

jwt.Jwt = Jwt

---@meta
---@diagnostic disable: lowercase-global, missing-return
---Crypto Module.
crypto = {}

---Hash a string using SHA-224.
---@param string string
---@return string hashed string
function crypto.sha224(string) end

---Hash a string using SHA-256.
---@param string string
---@return string hashed string
function crypto.sha256(string) end

---Hash a string using SHA-512/224.
---@param string string
---@return string hashed string
function crypto.sha512_224(string) end

---Hash a string using SHA-512/256.
---@param string string
---@return string hashed string
function crypto.sha512_256(string) end

---Hash a string using SHA-384.
---@param string string
---@return string hashed string
function crypto.sha384(string) end

---Hash a string using SHA-512.
---@param string string
---@return string hashed string
function crypto.sha512(string) end

---@class RsaPrivateKey Rsa private key.
local RsaPrivateKey = {}

---Create a new RSA private key.
---@param bits integer number of bits to use
---@return RsaPrivateKey
function RsaPrivateKey.new(bits) end

---Create a new RSA private key from a PEM file.
---@param string string PEM string
---@param type? "PKCS1" | "PKCS8" Type of PEM, defaults to PKCS8
---@return RsaPrivateKey
function RsaPrivateKey.from_pem(string, type) end

---Convert RSA private key to a PEM string.
---@param type? "PKCS1" | "PKCS8" Type of PEM, defaults to PKCS8
---@return string # PEM string
function RsaPrivateKey:to_pem(type) end

---Decrypt data using the private key.
---@param data integer[] data to encrypt.
---@return integer[] # decrypted data
function RsaPrivateKey:decrypt(data) end

---Generate a public key from the private key.
---@return RsaPublicKey # public key
function RsaPrivateKey:public_key() end

---@class RsaPublicKey Rsa public key.
local RsaPublicKey = {}

---Encrypt data using the public key.
---@param data integer[] data to encrypt.
---@return integer[] # encrypted data
function RsaPublicKey:encrypt(data) end

---@class Argon2
---Argon2 is a memory-hard key derivation function chosen as the winner of the
---Password Hashing Competition in July 2015.
local Argon2 = {}

---Create a new Argon password hasher.
---@param algorithm "Argon2d" | "Argon2i" | "Argon2id" Algorithmic variant of argon2.
---@return Argon2
function Argon2.new(algorithm) end

---Hash a password.
---@param password string password to hash.
---@return string hashed password.
function Argon2:hash(password) end

---Verify a password.
---@param hash string hashed password.
---@param password string password to verify.
---@return boolean
function Argon2:verify(hash, password) end

crypto.Argon2 = Argon2
crypto.RsaPrivateKey = RsaPrivateKey

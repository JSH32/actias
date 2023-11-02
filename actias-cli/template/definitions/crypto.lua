---@meta
---@diagnostic disable: lowercase-global, missing-return
---Crypto Module.
crypto = {}

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

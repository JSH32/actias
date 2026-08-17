---@meta

---Declare a secret this script uses and get its plaintext value.
---This is a declaration: it is only available at the top level of the
---entry point, and the declared names form the script's capability contract.
---Values are set with `actias secret put` and stored encrypted.
---@param name string name of the secret.
---@return string # the secret's value.
function secret(name) end

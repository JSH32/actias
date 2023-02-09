---@meta
---@diagnostic disable: lowercase-global

---@class WasmInstance

---Wasm instance.
wasm = {}

---Make a Webassembly runtime instance from a wasm binary.
---@param binary number[] Webassembly binary.
---@return WasmInstance
wasm.from = function(binary) end
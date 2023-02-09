---@meta
---@diagnostic disable: lowercase-global

---Global object from Webassembly.
---@class WasmGlobal
WasmGlobal = {}

---Get the current value of the global.
---@return any
function WasmGlobal:get() end

---Set the current value of the global
---@param value any new parameter value.
function WasmGlobal:set(value) end

---Instance of a Webassembly binary.
---@class WasmInstance
WasmInstance = {}

---Make a Webassembly runtime instance from a wasm binary.
---@param binary number[] Webassembly binary.
---@return WasmInstance
WasmInstance.from = function (binary) end

---Get a function from the Webassembly module.
---@param export string name of the exported function.
---@return function
function WasmInstance:get_function(export) end

---Get a global from the Webassembly module.
---@param global string name of the exported global.
---@return WasmGlobal
function WasmInstance:get_global(global) end

---Wasm module.
wasm = {}
wasm.WasmInstance = WasmInstance

---Make a Webassembly runtime instance from a wasm binary.
---@param binary number[] Webassembly binary.
---@return WasmInstance
wasm.from = function(binary) end
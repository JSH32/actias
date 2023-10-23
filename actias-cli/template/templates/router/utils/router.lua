---@class Router
---@field routes table<string, Route> Routes.
local Router = {}

Router.__index = Router
Router.validators = {}

---@return Router
function Router.new()
    local self = setmetatable({}, Router)
    self.routes = {}
    self.errorHandler = function(errors)
        return {
            status_code = 400,
            body = json.stringify({errors = errors, status_code = 400}),
            headers = {["Content-Type"] = "application/json"}
        }
    end
    return self
end

function Router:use()
    add_event_listener("fetch",
                       function(request) return self:route_request(request) end)
end

local function make_validator(schema)
    local validatorFns = {}
    for validatorName, validatorArgs in pairs(schema) do
        local validatorFn = assert(Router.validators[validatorName],
                                   "Validator '" .. validatorName ..
                                       "' is not defined")
        validatorFns[validatorName] = function(value)
            return validatorFn(value, validatorArgs)
        end
    end
    return validatorFns
end

---@class Route Route configuration.
---@field validators table<string, any>? Validator configuration.
---@field path string Path to match for the route.
---@field handler fun(request: RouterRequest): Response Handler for the route.
---@field method HttpMethod Method to match for the route.

---@class RouterRequest Request object from the router.
---@field request Request the raw request object.
---@field params table<string, string> Parsed URL params.
---@field query table<string, string> Parsed query parameters.

local function parse_path(path)
    -- e.g.: path = "/user/<id>"

    -- Splitting the path into segments
    local segments = {}
    for segment in string.gmatch(path, "/([^/]+)") do
        table.insert(segments, segment)
    end

    -- Parameter interpretation
    local parameters = {}
    for i, segment in ipairs(segments) do
        if string.sub(segment, 1, 1) == "<" and string.sub(segment, -1) == ">" then
            parameters[string.sub(segment, 2, -2)] = i
        end
    end

    return segments, parameters
end

local function match_path(request_path, route_path)
    -- e.g.: request_path = "/user/123", route_path = "/user/<id>"

    local route_segments, route_parameters = parse_path(route_path)
    local request_segments = {}
    for segment in string.gmatch(request_path, "%w+") do
        table.insert(request_segments, segment)
    end

    if #request_segments ~= #route_segments then return false end

    -- Extract parameter values
    local parameter_values = {}
    for i = 1, #route_segments do
        local route_segment = route_segments[i]
        local request_segment = request_segments[i]

        local start_pos, end_pos = string.find(route_segment, "<.->")
        if start_pos then
            local parameter_name = string.sub(route_segment, start_pos + 1,
                                              end_pos - 1)
            parameter_values[parameter_name] = request_segment
        elseif route_segment ~= request_segment then
            return false
        end
    end

    return true, parameter_values
end

---@param query string? The query string.
local function parse_query_string(query)
    local params = {}

    if query ~= nil then
        for param in string.gmatch(query, "([^&]+)") do
            local key, value = string.match(param, "(.*)=(.*)")
            if key and value then params[key] = value end
        end
    end

    return params
end

---Add a route to the router.
---@param route Route
function Router:add_route(route)
    if route.validators then
        for k, v in pairs(route.validators) do
            route.validators[k] = make_validator(v)
        end
    end
    table.insert(self.routes, route)
end

---@param handler fun(errors: string[]): Response Handler for validation errors.
function Router:setErrorHandler(handler) self.errorHandler = handler end

---Validate the request with respect to registered routes.
---@param request Request
---@param route Route
---@return boolean, string[] # true if the request is valid, otherwise false and list of errors.
function Router:validate_request(request, route)
    local errors = {}
    local body_params = {}

    if request.body ~= nil and #request.body ~= 0 then
        body_params = json.parse(request.body)
    end

    if route.validators then
        for paramName, validators in pairs(route.validators) do
            local value = body_params[paramName]

            if value == nil then
                table.insert(errors, "Field '" .. paramName .. "' is missing.")
            else
                for validatorName, validatorFn in pairs(validators) do
                    local isValid, errMsg = validatorFn(value)
                    if not isValid then
                        errMsg = "Field '" .. paramName .. "': " .. errMsg
                        table.insert(errors, errMsg)
                        break
                    end
                end
            end
        end
    end

    if #errors > 0 then return false, errors end

    return true, {}
end

---Route the request to the respective routes or return an error.
---@param request Request raw HTTP request.
---@return Response # from the respective handler.
function Router:route_request(request)
    local uri_parts = Uri.parse(request.context_uri)
    for _, route in pairs(self.routes) do
        if route.method == request.method then
            local match, parameters = match_path(uri_parts.path, route.path)
            if match then
                local ok, errors = self:validate_request(request, route)
                if not ok then return self.errorHandler(errors) end

                return route.handler({
                    request = request,
                    params = parameters or {},
                    query = parse_query_string(uri_parts.query)
                })
            end
        end
    end

    return {status_code = 404, body = "Not Found"}
end

Router.validators.isLength = function(value, options)
    local len = #value
    if len < options.min then
        return false, "Length is less than the minimum required (" ..
                   options.min .. ")."
    elseif options.max and len > options.max then
        return false,
               "Length exceeds the maximum allowed (" .. options.max .. ")."
    end
    return true
end

Router.validators.isType = function(value, expectedType)
    if type(value) ~= expectedType then
        return false, "Expected type '" .. expectedType .. "', but got '" ..
                   type(value) .. "'."
    end
    return true
end

return Router

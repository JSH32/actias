---@diagnostic disable: lowercase-global, missing-return

---@alias HttpMethod
---| "GET"|"POST"|"PUT"|"DELETE"|"HEAD"|"OPTIONS"|"CONNECT"|"PATCH"|"TRACE"

---@class Request Represents an HTTP request.
---@field url string URL to send the request.
---@field context_uri? string URL without identifier, use for routing. This is ignored when making requests.
---@field method? HttpMethod request method
---@field headers? table<string, string> HTTP headers.
---@field body? string request body.
---@field version? string request version.

---@class Response Represents an HTTP response.
---@field status_code? integer HTTP status code, otherwise 200.
---@field headers? table<string, string> HTTP headers.
---@field body? string request body.

---Exposed HTTP module.
http = {}

---Make a request with the parameters.
---@param request Request request parameters.
---@return Response
http.make_request = function(request) end
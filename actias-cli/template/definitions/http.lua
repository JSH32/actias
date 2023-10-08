---@meta
---@diagnostic disable: lowercase-global, missing-return

---@alias HttpMethod
---| "GET"|"POST"|"PUT"|"DELETE"|"HEAD"|"OPTIONS"|"CONNECT"|"PATCH"|"TRACE"

---@class Uri Parse a URI into parts.
---@field scheme string URI scheme.
---@field authority string URI authority.
---@field path string URI path.
---@field query string Query parameters.
Uri = {}

---Parse a lua string into a URI.
---@param string string string to parse into URI.
function Uri.parse(string) end

---Convert a URI to a string.
function Uri:tostring() end

---@class Request Represents an HTTP request.
---@field uri string URL to send the request.
---@field context_uri string URL without identifier, use for routing. This is ignored when making requests.
---@field method HttpMethod request method
---@field headers table<string, string> HTTP headers.
---@field body string request body.
---@field version string request version.

---@class Response Represents an HTTP response.
---@field status_code integer HTTP status code, otherwise 200.
---@field headers table<string, string> HTTP headers.
---@field body string request body.

---Exposed HTTP module.
http = {}

http.Uri = Uri

---Make a request with the parameters.
---@param request Request request parameters.
---@return Response
http.make_request = function(request) end

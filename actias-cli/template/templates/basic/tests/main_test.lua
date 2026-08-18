-- `actias test` runs these on the same runtime the platform uses; `fetch`
-- dispatches to the handler main.lua registered, and kv/secrets are faked
-- in memory (values for secrets come from tests/secrets.json).
test("responds with the greeting", function()
    local response = fetch({ method = "GET", path = "/" })
    local body = json.parse(response.body)

    assert(body.hello == "world", "expected the greeting")
    assert(response.headers["Content-Type"] == "application/json")
end)

add_event_listener("fetch", function(request)
    return {
        body = json.stringify({hello = "there"}),
        headers = {["Content-Type"] = "application/json"}
    }
end)

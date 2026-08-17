-- `on` declares an event handler; declarations live at the top level.
on "fetch" (function(request)
    return {
        body = json.stringify({hello = "world"}),
        headers = {["Content-Type"] = "application/json"}
    }
end)

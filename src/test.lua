add_event_listener("fetch", function(request)
    local hi = request.body:text()

    return {
        status_code = 200,
        body = request.version,
    }
end)
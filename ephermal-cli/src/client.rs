use progenitor::generate_api;

generate_api!(
    spec = "src/ephermal-api.json", // The OpenAPI document
    interface = Positional,
);

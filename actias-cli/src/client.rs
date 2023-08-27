use progenitor::generate_api;

generate_api!(
    spec = "src/actias-api.json", // The OpenAPI document
    // interface = Positional,
    interface = Builder,
    derives = [schemars::JsonSchema],
);

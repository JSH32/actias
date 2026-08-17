// Everything in this module is macro-generated from the OpenAPI document;
// lints against its style are not actionable here.
#![allow(clippy::to_string_trait_impl)]

use progenitor::generate_api;

generate_api!(
    spec = "src/actias-api.json", // The OpenAPI document
    interface = Builder,
);

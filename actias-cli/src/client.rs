//! The api client, generated from `src/actias-api.json` by progenitor.
//! The document is the input; nothing here is hand-written, and it is
//! regenerated rather than edited (rules/70).

// Everything in this module is macro-generated from the OpenAPI document;
// lints against its style are not actionable here.
#![allow(clippy::to_string_trait_impl)]

use progenitor::generate_api;

generate_api!(
    spec = "src/actias-api.json", // The OpenAPI document
    interface = Builder,
);

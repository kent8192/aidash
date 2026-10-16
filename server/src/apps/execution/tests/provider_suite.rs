//! Provider adapters use native HTTP server and client fixtures.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "provider_fixtures.rs"]
mod provider_fixtures;
#[path = "provider_streams.rs"]
mod provider_streams;
#[path = "provider_timeouts.rs"]
mod provider_timeouts;
#[path = "providers.rs"]
mod providers;

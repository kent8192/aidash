//! App-local distributed protocol E2E tests using native framework fixtures.
#[path = "fixtures.rs"]
mod fixtures;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "protocol.rs"]
mod protocol;
#[path = "../../../execution/tests/support/settings.rs"]
mod settings;
#[path = "../../../execution/tests/support/state.rs"]
mod state;

//! Empty memory recall through a conversation member Run.
#[allow(dead_code, clippy::disallowed_methods)] // Shared fixture uses deployment-scoped setup.
#[path = "native_memory/fixture.rs"]
mod fixture;
#[path = "native_memory/memory_authority.rs"]
mod memory_authority;
#[allow(clippy::disallowed_methods)] // Database fixture establishes scoped connections.
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../execution/tests/support/upstream.rs"]
mod upstream;

use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::memory::Bounds;
use aidash_server::authorization::identity::Actor;
use fixture::*;
use native_database::{DatabaseFixture, database};
use reinhardt::ServerRouter as Router;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::{Arc, Mutex, atomic::AtomicUsize};
use upstream::reply;
use uuid::Uuid;

struct RuntimeControls {
	client: reqwest::Client,
	notify: Arc<tokio::sync::Notify>,
}

#[fixture]
fn runtime_controls(
	#[from(reinhardt::test::fixtures::http_client)] client: reqwest::Client,
) -> RuntimeControls {
	RuntimeControls {
		client,
		notify: Arc::new(tokio::sync::Notify::new()),
	}
}

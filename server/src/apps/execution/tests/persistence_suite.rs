#[path = "support/native_database.rs"]
mod native_database;
use native_database::{DatabaseFixture, database};

#[path = "../../knowledge/tests/database_limits.rs"]
mod semantic_limits;

#[path = "../../federation/transactions/tests/database_decisions.rs"]
mod decisions;

#[path = "../../workspaces/tests/database_task_graph.rs"]
mod task_graph;

#[path = "database_delivery.rs"]
mod delivery;

use aidash_server::apps::registry::models::Definition;
use aidash_server::apps::registry::services::states::DefinitionKind;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Model;
use reinhardt::model;
use rstest::{fixture, rstest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[fixture]
fn definition() -> Definition {
	Definition::build()
        .id("policy-test")
        .version("1.0.0")
        .kind(DefinitionKind::Skill)
        .metadata(json!({"id":"policy-test","version":"1.0.0","kind":"skill","name":{"en":"Test"},"description":{"en":"Test"},"capabilities":[],"tags":[],"languages":["en"],"skills":[],"schema":{},"config":{"instructions":"Test"}}).into())
        .finish()
}

#[rstest]
#[tokio::test]
async fn registry_natural_key_round_trips_through_the_orm(
	#[future] database: DatabaseFixture,
	definition: Definition,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let saved = Definition::objects()
		.create_with_conn(&mut connection, &definition)
		.await
		.unwrap();
	let loaded = Definition::objects()
		.filter(Definition::field_id().eq(definition.id.clone()))
		.filter(Definition::field_version().eq(definition.version.clone()))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(loaded, saved);
}

// Exercise a nullable production shape without changing the application's model inventory.
#[model(app_label = "test_probe", table_name = "dashboard_identities")]
#[derive(Serialize, Deserialize)]
struct NullableIdentity {
	#[field(primary_key = true)]
	id: Uuid,
	#[field(field_type = "text")]
	issuer: String,
	#[field(field_type = "text")]
	subject: String,
	#[field(null = true)]
	last_valid_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	disabled_at: Option<DateTime<Utc>>,
}

#[fixture]
fn nullable_identity() -> NullableIdentity {
	NullableIdentity::build()
		.issuer("fixture")
		.subject("alice")
		.last_valid_at(None)
		.disabled_at(None)
		.finish()
}

#[rstest]
#[tokio::test]
async fn nullable_timestamps_round_trip_through_the_orm(
	#[future] database: DatabaseFixture,
	nullable_identity: NullableIdentity,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let saved = NullableIdentity::objects()
		.create_with_conn(&mut connection, &nullable_identity)
		.await
		.unwrap();
	assert_eq!(saved.last_valid_at, None);
	assert_eq!(saved.disabled_at, None);
}

#[path = "database_leases.rs"]
mod leases;
#[path = "run_fixtures.rs"]
mod run_fixtures;
#[path = "database_waiting_requests.rs"]
mod waiting_requests;

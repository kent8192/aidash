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

#[rstest]
#[case::pool(false)]
#[case::transaction(true)]
#[tokio::test]
async fn native_null_parameters_retain_timestamp_and_uuid_types(
	#[future] database: DatabaseFixture,
	#[case] transactional: bool,
) {
	// Arrange: use real NULL parameters, not SQL literal substitution.
	use reinhardt::db::backends::types::QueryValue;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let database = database.await;
	// Query renders absent values inline; positional expressions exercise the
	// native backend's separate parameter contract without that substitution.
	let sql = Query::select()
		.expr_as(
			Expr::cust("$1").cast_as(Alias::new("timestamptz")),
			Alias::new("expires_at"),
		)
		.expr_as(
			Expr::cust("$2").cast_as(Alias::new("uuid")),
			Alias::new("request_id"),
		)
		.to_string(PostgresQueryBuilder);
	let parameters = vec![QueryValue::Null, QueryValue::Null];
	// Act: both native executor paths must infer the SQL context (#6631).
	let rows = if transactional {
		let mut transaction = database.connection.begin().await.unwrap();
		let rows = transaction.fetch_all(&sql, parameters).await.unwrap();
		transaction.rollback().await.unwrap();
		rows
	} else {
		database
			.connection
			.fetch_all(&sql, parameters)
			.await
			.unwrap()
	};
	// Assert: neither parameter is encoded as an integer or converted to JSON null.
	assert_eq!(rows.len(), 1);
	assert_eq!(rows[0].data.get("expires_at"), Some(&QueryValue::Null));
	assert_eq!(rows[0].data.get("request_id"), Some(&QueryValue::Null));
}

#[path = "database_leases.rs"]
mod leases;
#[path = "run_fixtures.rs"]
mod run_fixtures;
#[path = "database_waiting_requests.rs"]
mod waiting_requests;

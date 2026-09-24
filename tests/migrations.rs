// This suite only needs the shared database fixture, not the API helpers.
#[allow(dead_code)]
mod common;
use migration::{Migrator, MigratorTrait};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn seaorm_migrations_round_trip_a_fresh_schema() {
	let (f, url, schema) = common::setup().await;
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	assert_eq!(
		Migrator::get_applied_migrations(&db).await.unwrap().len(),
		Migrator::migrations().len()
	);
	Migrator::down(&db, None).await.unwrap();
	assert!(
		Migrator::get_applied_migrations(&db)
			.await
			.unwrap()
			.is_empty()
	);
	Migrator::up(&db, None).await.unwrap();
	f.store
		.create_workspace("After migration", "Schema was rebuilt")
		.await
		.unwrap();
	common::cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn run_input_migration_preserves_keyed_message_retries() {
	let (f, url, schema) = common::setup().await;
	let app = aidash::api::router(f.clone());
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = common::request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Migration retry", "goal":"Reply", "target":{"id":"research","version":"1.0.0"}, "target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	// Remove the run-input migrations to simulate a message written before
	// the ledger existed, then apply the backfill and delivery extensions.
	Migrator::down(&db, Some(4)).await.unwrap();
	let retry_key = Uuid::new_v4();
	let key = format!("subject-human:acme:alice:{}:{retry_key}", run.id);
	f.store
		.message(
			run.workspace_id,
			"alice",
			"accepted before upgrade",
			Some(&key),
		)
		.await
		.unwrap();
	let large_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.message(
			run.workspace_id,
			"human",
			&"historical correction ".repeat(1500),
			Some(&large_key),
		)
		.await
		.unwrap();
	Migrator::up(&db, None).await.unwrap();
	let inputs = f.store.run_inputs(run.id).await.unwrap();
	assert_eq!(inputs.len(), 2);
	assert!(
		inputs
			.iter()
			.any(|input| input.idempotency_key == key && input.message_id.is_some())
	);
	let admitted_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(
			run.id,
			"human",
			"valid after upgrade",
			&admitted_key,
			f.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	assert!(
		f.store
			.run_inputs(run.id)
			.await
			.unwrap()
			.iter()
			.any(|input| input.idempotency_key == large_key && input.reference_only)
	);
	let old_replica_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	assert!(
		f.store
			.message(
				run.workspace_id,
				"human",
				"old replica lacks admission",
				Some(&old_replica_key)
			)
			.await
			.is_err()
	);
	assert!(
		f.store
			.message(run.workspace_id, "alice", "unkeyed old correction", None)
			.await
			.is_err()
	);
	let workspace_message_path = format!("/api/workspaces/{}/messages", run.workspace_id);
	assert_eq!(
		common::request(
			&app,
			&token,
			"POST",
			&workspace_message_path,
			json!({"content":"new workspace message"})
		)
		.await
		.0,
		200
	);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'COMPLETED'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let rejected_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	assert!(
		f.store
			.message(
				run.workspace_id,
				"human",
				"rejected by the bridge",
				Some(&rejected_key)
			)
			.await
			.is_err()
	);
	assert!(
		!f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.idempotency_key.as_deref() == Some(&rejected_key))
	);
	let path = format!("/api/runs/{}/message", run.id);
	assert_eq!(
		common::request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"accepted before upgrade","idempotency_key":retry_key})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		common::request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"new after completion","idempotency_key":Uuid::new_v4()})
		)
		.await
		.0,
		409
	);
	common::cleanup(f, &url, &schema).await;
}

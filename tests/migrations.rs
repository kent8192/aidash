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
	// Remove both run-input migrations to simulate a message written before
	// the ledger existed, then apply the backfill and delivery extensions.
	Migrator::down(&db, Some(2)).await.unwrap();
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
	Migrator::up(&db, None).await.unwrap();
	let inputs = f.store.run_inputs(run.id).await.unwrap();
	assert_eq!(inputs.len(), 1);
	assert_eq!(inputs[0].idempotency_key, key);
	assert!(inputs[0].message_id.is_some());
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

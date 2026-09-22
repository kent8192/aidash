#[allow(dead_code)]
mod common;

use aidash::registry::{Entry, ModelConfig};
use common::{cleanup, request, setup};
use migration::MigratorTrait;
use sea_orm::sea_query::{Alias, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};

fn model(id: &str) -> Entry {
	serde_json::from_value(json!({
		"id":id, "version":"1.0.0", "kind":"model",
		"name":{"en":"Timeout fixture"}, "description":{"en":"Persistence test"},
		"config":{"provider":"openrouter", "model_id":"vendor/model",
			"endpoint":"http://localhost:9999/v1", "credential_env":null,
			"context_window":32768, "max_output_tokens":4096,
			"modalities":["text"], "cost":{}}
	}))
	.unwrap()
}

async fn insert_model(pool: &sqlx::PgPool, entry: &Entry) -> Result<(), sqlx::Error> {
	let query = Query::insert()
		.into_table(Alias::new("registry"))
		.columns(["id", "version", "kind", "metadata"].map(Alias::new))
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(&entry.id)
		.bind(&entry.version)
		.bind(&entry.kind)
		.bind(json!(entry))
		.execute(pool)
		.await
		.map(|_| ())
}

async fn install(pool: &sqlx::PgPool, config: &Value) -> Result<(), sqlx::Error> {
	let query = Query::insert()
		.into_table(Alias::new("installations"))
		.columns(["id", "version", "digest", "config"].map(Alias::new))
		.values_panic([
			Expr::val("timeout-model"),
			Expr::val("1.0.0"),
			Expr::val("fixture"),
			Expr::cust("$1"),
		])
		.on_conflict(
			OnConflict::columns([Alias::new("id"), Alias::new("version")])
				.value(Alias::new("config"), Expr::cust("$1"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(config)
		.execute(pool)
		.await
		.map(|_| ())
}

fn rejected(result: Result<(), sqlx::Error>, constraint: &str) {
	let error = result.unwrap_err();
	let database = error.as_database_error().expect("database constraint error");
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some(constraint), "{error}");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn configured_timeouts_round_trip_through_the_registry_api() {
	let (f, url, schema) = setup().await;
	let app = aidash::api::router(f.clone());
	for (index, timeout) in [None, Some(Value::Null), Some(json!(1)), Some(json!(900)), Some(json!(1200)), Some(json!(u32::MAX))]
		.into_iter()
		.enumerate()
	{
		let mut entry = model(&format!("model-{index}"));
		if let Some(timeout) = timeout {
			entry.config["request_timeout_secs"] = timeout;
		}
		let expected = entry.config.clone();
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry",
			json!(entry),
		)
		.await;
		assert_eq!(status, 200, "model registration: {body}");
		let stored = f.registry.get(&entry.id, &entry.version).await.unwrap();
		assert_eq!(stored.config, expected);
		let config: ModelConfig = serde_json::from_value(stored.config).unwrap();
		assert_eq!(
			config.request_timeout().unwrap().as_secs(),
			expected["request_timeout_secs"].as_u64().unwrap_or(900)
		);
	}
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn database_validates_registered_and_overridden_timeouts() {
	let (f, url, schema) = setup().await;
	f.registry.register(model("timeout-model")).await.unwrap();
	for timeout in [Value::Null, json!(1), json!(900), json!(1200), json!(u32::MAX)] {
		let config = json!({"request_timeout_secs":timeout});
		install(&f.store.pool, &config).await.unwrap();
		let stored = f.registry.get("timeout-model", "1.0.0").await.unwrap();
		assert_eq!(stored.config["request_timeout_secs"], timeout);
	}
	for (index, timeout) in [json!(0), json!(-1), json!(1.5), json!("900"), json!(true), json!([]), json!({}), json!(4294967296_u64)]
		.into_iter()
		.enumerate()
	{
		let mut entry = model(&format!("invalid-{index}"));
		entry.config["request_timeout_secs"] = timeout.clone();
		rejected(insert_model(&f.store.pool, &entry).await, "registry_model_config");
		rejected(
			install(&f.store.pool, &json!({"request_timeout_secs":timeout})).await,
			"installations_config",
		);
	}
	let mut unknown = model("unknown-field");
	unknown.config["request_timeout_sec"] = json!(900);
	rejected(insert_model(&f.store.pool, &unknown).await, "registry_model_config");
	rejected(
		install(&f.store.pool, &json!({"request_timeout_sec":900})).await,
		"installations_config",
	);
	install(&f.store.pool, &json!({})).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn timeout_migration_upgrades_existing_models_and_preserves_rollback_safety() {
	let (f, url, schema) = setup().await;
	let migrations = migration::Migrator::migrations();
	assert_eq!(migrations.last().unwrap().name(), "m20260922_134000_inference_timeout_constraints");
	migration::Migrator::down(&f.registry.db, Some(1)).await.unwrap();
	let legacy = model("timeout-model");
	f.registry.register(legacy.clone()).await.unwrap();
	install(&f.store.pool, &json!({})).await.unwrap();
	let mut configured = model("configured");
	configured.config["request_timeout_secs"] = json!(900);
	rejected(insert_model(&f.store.pool, &configured).await, "registry_model_config");
	rejected(
		install(&f.store.pool, &json!({"request_timeout_secs":1200})).await,
		"installations_config",
	);
	migration::Migrator::up(&f.registry.db, None).await.unwrap();
	assert_eq!(f.registry.get("timeout-model", "1.0.0").await.unwrap().config, legacy.config);
	// Downgrade and re-upgrade without new fields must preserve existing rows.
	migration::Migrator::down(&f.registry.db, Some(1)).await.unwrap();
	migration::Migrator::up(&f.registry.db, None).await.unwrap();
	install(&f.store.pool, &json!({"request_timeout_secs":1200})).await.unwrap();
	// Never silently discard a timeout during downgrade.
	assert!(migration::Migrator::down(&f.registry.db, Some(1)).await.is_err());
	assert_eq!(f.registry.get("timeout-model", "1.0.0").await.unwrap().config["request_timeout_secs"], 1200);
	f.registry.register(configured).await.unwrap();
	cleanup(f, &url, &schema).await;
}

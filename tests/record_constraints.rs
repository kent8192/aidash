#[allow(dead_code)]
mod common;

use aidash::{domain::NewTask, registry::Entry};
use common::{cleanup, setup};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::{Value, json};

fn model() -> Entry {
	serde_json::from_value(json!({
		"id":"test-model", "version":"1.0.0", "kind":"model",
		"name":{"en":"Test model"}, "description":{"en":"Fixture"},
		"config":{"provider":"openrouter", "model_id":"vendor/model",
			"endpoint":"http://localhost:9999/v1", "credential_env":null,
			"context_window":4096, "modalities":["text"], "cost":{}}
	}))
	.unwrap()
}

async fn insert_entry(pool: &sqlx::PgPool, entry: &Value) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("registry"))
			.columns(["id", "version", "kind", "metadata"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(entry["id"].as_str().unwrap())
	.bind(entry["version"].as_str().unwrap())
	.bind(entry["kind"].as_str().unwrap())
	.bind(entry)
	.execute(pool)
	.await
	.map(|_| ())
}

fn check_rejected(result: Result<(), sqlx::Error>, name: &str) {
	let error = result.unwrap_err();
	let database = error
		.as_database_error()
		.expect("database constraint error");
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some(name), "{error}");
}

async fn update(
	pool: &sqlx::PgPool,
	table: &str,
	column: &str,
	value: impl Into<SimpleExpr>,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::update()
			.table(Alias::new(table))
			.value(Alias::new(column), value)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.map(|_| ())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn registry_constraints_reject_invalid_models_without_application_validation() {
	let (f, url, schema) = setup().await;
	let good = serde_json::to_value(model()).unwrap();
	for (field, invalid) in [
		("provider", json!("unsupported")),
		("model_id", json!("   ")),
		("endpoint", Value::Null),
		("context_window", json!(0)),
		("context_window", json!(2048.5)),
		("context_window", json!("4096")),
		("modalities", json!(["audio"])),
		("reasoning_effort", json!("extreme")),
	] {
		let mut entry = good.clone();
		entry["config"][field] = invalid;
		check_rejected(
			insert_entry(&f.store.pool, &entry).await,
			"registry_model_config",
		);
	}
	let mut missing = good.clone();
	missing["config"]
		.as_object_mut()
		.unwrap()
		.remove("model_id");
	check_rejected(
		insert_entry(&f.store.pool, &missing).await,
		"registry_model_config",
	);
	let mut malformed = good.clone();
	malformed["name"] = json!([]);
	check_rejected(
		insert_entry(&f.store.pool, &malformed).await,
		"registry_metadata_shape",
	);
	for version in ["latest", "01.0.0", "1.0.0-01"] {
		let mut entry = good.clone();
		entry["version"] = json!(version);
		check_rejected(insert_entry(&f.store.pool, &entry).await, "registry_semver");
	}
	insert_entry(&f.store.pool, &good).await.unwrap();
	// An alias and a distinct version are intentionally valid; names/config are not unique keys.
	let mut alias = good.clone();
	alias["id"] = json!("alias");
	insert_entry(&f.store.pool, &alias).await.unwrap();
	alias["version"] = json!("1.1.0-rc.1+build.01");
	insert_entry(&f.store.pool, &alias).await.unwrap();
	check_rejected(
		update(&f.store.pool, "registry", "id", Expr::val("mismatch")).await,
		"registry_identity",
	);
	let mut whitespace = model();
	whitespace.config["model_id"] = json!("   ");
	assert!(matches!(
		aidash::registry::validate(&whitespace),
		Err(aidash::Error::Invalid(_))
	));
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn workspace_task_and_run_constraints_preserve_local_and_remote_boundaries() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let other = f.store.create_workspace("Other", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let task = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let foreign = f
		.store
		.create_task(other.id, &input, "human", None)
		.await
		.unwrap();
	check_rejected(
		update(&f.store.pool, "workspaces", "revision", Expr::val(-1)).await,
		"workspaces_content",
	);
	check_rejected(
		update(
			&f.store.pool,
			"workspaces",
			"state",
			Expr::cust("'[]'::jsonb"),
		)
		.await,
		"workspaces_content",
	);
	check_rejected(
		update(&f.store.pool, "tasks", "title", Expr::val(" ")).await,
		"tasks_content",
	);
	check_rejected(
		update(
			&f.store.pool,
			"tasks",
			"parent_id",
			Expr::col(Alias::new("id")),
		)
		.await,
		"tasks_no_self_reference",
	);
	check_rejected(
		update(
			&f.store.pool,
			"tasks",
			"dependencies",
			Expr::cust("ARRAY[id]"),
		)
		.await,
		"tasks_no_self_reference",
	);
	let result = sqlx::query(
		&Query::update()
			.table(Alias::new("tasks"))
			.value(Alias::new("parent_id"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(foreign.id)
	.bind(task.id)
	.execute(&f.store.pool)
	.await
	.unwrap_err();
	assert_eq!(
		result.as_database_error().unwrap().constraint(),
		Some("tasks_parent_workspace")
	);
	let artifact = Query::insert()
		.into_table(Alias::new("artifacts"))
		.columns(
			[
				"id",
				"workspace_id",
				"task_id",
				"kind",
				"name",
				"content",
				"created_by",
				"idempotency_key",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("gen_random_uuid()"),
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::val("text").into(),
			Expr::val("Result").into(),
			Expr::cust("'{}'::jsonb"),
			Expr::val("human").into(),
			Expr::val("artifact-key").into(),
		])
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&artifact)
		.bind(other.id)
		.bind(task.id)
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	assert_eq!(
		error.as_database_error().unwrap().constraint(),
		Some("artifacts_task_workspace")
	);
	sqlx::query(&artifact)
		.bind(workspace.id)
		.bind(task.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	// Remote runs are allowed even if their workspace/task only exist at their home node.
	let mut remote = task.clone();
	remote.id = uuid::Uuid::new_v4();
	remote.workspace_id = uuid::Uuid::new_v4();
	let run = f
		.store
		.accept_run(&remote, "aidash://remote", "executor", "1.0.0")
		.await
		.unwrap();
	let human_request = Query::insert()
		.into_table(Alias::new("human_requests"))
		.columns(
			[
				"id",
				"workspace_id",
				"run_id",
				"kind",
				"prompt",
				"request_key",
			]
			.map(Alias::new),
		)
		.values_panic([
			Expr::cust("gen_random_uuid()"),
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::val("QUESTION").into(),
			Expr::val("Continue?").into(),
			Expr::val("question-key").into(),
		])
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&human_request)
		.bind(workspace.id)
		.bind(run.id)
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	assert_eq!(
		error.as_database_error().unwrap().constraint(),
		Some("human_requests_run_workspace")
	);
	sqlx::query(&human_request)
		.bind(remote.workspace_id)
		.bind(run.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	check_rejected(
		update(&f.store.pool, "runs", "step", Expr::val(-1)).await,
		"runs_counters",
	);
	check_rejected(
		update(
			&f.store.pool,
			"runs",
			"lease_owner",
			Expr::cust("gen_random_uuid()"),
		)
		.await,
		"runs_lease",
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn constraints_upgrade_and_rollback_preserve_data_and_reject_invalid_history() {
	use migration::MigratorTrait;
	let (f, url, schema) = setup().await;
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	let good = serde_json::to_value(model()).unwrap();
	insert_entry(&f.store.pool, &good).await.unwrap();
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	assert_eq!(
		f.registry.get("test-model", "1.0.0").await.unwrap(),
		model()
	);
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	let mut invalid = good.clone();
	invalid["id"] = json!("invalid");
	invalid["config"]["context_window"] = json!(0);
	insert_entry(&f.store.pool, &invalid).await.unwrap();
	assert!(migration::Migrator::up(&db, None).await.is_err());
	// The migration must never silently repair/delete user data.
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("registry"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(count, 2);
	// Correcting the bad record lets a failed, transactionally rolled-back upgrade retry.
	sqlx::query(
		&Query::update()
			.table(Alias::new("registry"))
			.value(Alias::new("metadata"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("id")).eq("invalid"))
			.to_string(PostgresQueryBuilder),
	)
	.bind({
		invalid["config"]["context_window"] = json!(4096);
		invalid
	})
	.execute(&f.store.pool)
	.await
	.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	cleanup(f, &url, &schema).await;
}

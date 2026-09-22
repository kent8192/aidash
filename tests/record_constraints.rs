#[allow(dead_code)]
mod common;

use aidash::{domain::NewTask, registry::Entry};
use common::{cleanup, setup};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

fn check_foreign_key_rejected(result: Result<(), sqlx::Error>, name: &str) {
	let error = result.unwrap_err();
	let database = error
		.as_database_error()
		.expect("database foreign-key error");
	assert_eq!(database.code().as_deref(), Some("23503"), "{error}");
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

async fn update_package_manifest(
	pool: &sqlx::PgPool,
	id: &str,
	version: &str,
	manifest: Value,
) -> Result<(), sqlx::Error> {
	let source = manifest.to_string();
	let digest = aidash::registry::digest(&manifest);
	sqlx::query(
		&Query::update()
			.table(Alias::new("packages"))
			.value(Alias::new("manifest"), Expr::cust("$1"))
			.value(Alias::new("manifest_source"), Expr::cust("$2"))
			.value(Alias::new("digest"), Expr::cust("$3"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$4")))
			.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$5")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(manifest)
	.bind(source)
	.bind(digest)
	.bind(id)
	.bind(version)
	.execute(pool)
	.await
	.map(|_| ())
}

async fn update_package_digest(
	pool: &sqlx::PgPool,
	id: &str,
	version: &str,
	digest: &str,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("packages"))
			.value(Alias::new("digest"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$3")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(digest)
	.bind(id)
	.bind(version)
	.execute(pool)
	.await
	.map(|_| ())
}

async fn update_package_source(
	pool: &sqlx::PgPool,
	id: &str,
	version: &str,
	source: &str,
	digest: &str,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("packages"))
			.value(Alias::new("manifest_source"), Expr::cust("$1"))
			.value(Alias::new("digest"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$3")))
			.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$4")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(source)
	.bind(digest)
	.bind(id)
	.bind(version)
	.execute(pool)
	.await
	.map(|_| ())
}

async fn update_registry_config(
	pool: &sqlx::PgPool,
	id: &str,
	version: &str,
	config: Value,
) -> Result<(), sqlx::Error> {
	let select = Query::select()
		.column(Alias::new("metadata"))
		.from(Alias::new("registry"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let (mut metadata,): (Value,) = sqlx::query_as(&select)
		.bind(id)
		.bind(version)
		.fetch_one(pool)
		.await?;
	metadata["config"] = config;
	let update = Query::update()
		.table(Alias::new("registry"))
		.value(Alias::new("metadata"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.and_where(Expr::col(Alias::new("version")).eq(Expr::cust("$3")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.bind(metadata)
		.bind(id)
		.bind(version)
		.execute(pool)
		.await
		.map(|_| ())
}

async fn update_run_state(
	pool: &sqlx::PgPool,
	run_id: uuid::Uuid,
	phase: &str,
	pending: Value,
) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), Expr::cust("$1"))
			.value(Alias::new("pending"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$3")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(phase)
	.bind(pending)
	.bind(run_id)
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
	for endpoint in [
		"ftp://localhost/v1",
		"http:///missing-host",
		"http://user:pass@localhost/v1",
		"https://localhost/v1?token=x",
		"https://localhost/v1#fragment",
	] {
		let mut entry = good.clone();
		entry["config"]["endpoint"] = json!(endpoint);
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
	let set_task_revision = || {
		Query::update()
			.table(Alias::new("tasks"))
			.value(Alias::new("revision"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder)
	};
	check_rejected(
		sqlx::query(&set_task_revision())
			.bind(i64::MAX)
			.bind(task.id)
			.execute(&f.store.pool)
			.await
			.map(|_| ()),
		"tasks_content",
	);
	sqlx::query(&set_task_revision())
		.bind(i64::MAX - 1)
		.bind(task.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	check_rejected(
		sqlx::query(&set_task_revision())
			.bind(i64::MAX)
			.bind(task.id)
			.execute(&f.store.pool)
			.await
			.map(|_| ()),
		"tasks_content",
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn nonblank_constraints_match_rust_unicode_whitespace() {
	let (f, url, schema) = setup().await;
	insert_entry(&f.store.pool, &serde_json::to_value(model()).unwrap())
		.await
		.unwrap();
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	f.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Task".into(),
				description: "Work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	// Derive the complete set independently from Rust, including NBSP and separators.
	let whitespace: Vec<_> = (0..=0x10ffff)
		.filter_map(char::from_u32)
		.filter(|c| c.is_whitespace())
		.collect();
	let mut blanks: Vec<_> = whitespace.iter().map(char::to_string).collect();
	blanks.push(whitespace.iter().collect());
	blanks.push(String::new());
	for blank in blanks {
		assert!(blank.trim().is_empty());
		for (kind, field, constraint) in [
			("model", "model_id", "registry_model_config"),
			("model", "endpoint", "registry_model_config"),
			("agent", "instructions", "registry_agent_config"),
			("skill", "instructions", "registry_skill_config"),
		] {
			let mut entry = serde_json::to_value(model()).unwrap();
			entry["kind"] = json!(kind);
			entry["config"][field] = json!(blank);
			check_rejected(insert_entry(&f.store.pool, &entry).await, constraint);
		}
		for (table, column, constraint) in [
			("workspaces", "title", "workspaces_content"),
			("workspaces", "goal", "workspaces_content"),
			("tasks", "title", "tasks_content"),
			("tasks", "description", "tasks_content"),
		] {
			check_rejected(
				update(&f.store.pool, table, column, Expr::val(blank.clone())).await,
				constraint,
			);
		}
	}
	// Whitespace surrounding real content and non-whitespace Unicode remain valid.
	for (index, content) in ["\t\u{a0}model\u{3000}\n", "\u{200b}"].iter().enumerate() {
		assert!(!content.trim().is_empty());
		for kind in ["model", "agent", "skill"] {
			let mut entry = serde_json::to_value(model()).unwrap();
			entry["id"] = json!(format!("{kind}-{index}"));
			entry["kind"] = json!(kind);
			if kind == "model" {
				entry["config"]["model_id"] = json!(content);
			} else {
				entry["config"] =
					json!({"instructions":content, "model":{"id":"test-model", "version":"1.0.0"}});
			}
			insert_entry(&f.store.pool, &entry).await.unwrap();
		}
		for (table, column) in [
			("workspaces", "title"),
			("workspaces", "goal"),
			("tasks", "title"),
			("tasks", "description"),
		] {
			update(&f.store.pool, table, column, Expr::val(*content))
				.await
				.unwrap();
		}
	}
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn localized_metadata_requires_string_values_for_every_locale() {
	let (f, url, schema) = setup().await;
	for field in ["name", "description"] {
		for invalid in [
			json!(7),
			json!(true),
			Value::Null,
			json!([]),
			json!(["text"]),
			json!({"nested":"text"}),
		] {
			let mut entry = serde_json::to_value(model()).unwrap();
			entry[field] = json!({"en":"Valid", "ja":invalid});
			check_rejected(
				insert_entry(&f.store.pool, &entry).await,
				"registry_metadata_shape",
			);
		}
	}
	let mut entry = model();
	entry.name.insert("ja".into(), "モデル".into());
	entry.description.insert("ja".into(), "説明".into());
	insert_entry(&f.store.pool, &serde_json::to_value(&entry).unwrap())
		.await
		.unwrap();
	assert_eq!(
		f.registry.get(&entry.id, &entry.version).await.unwrap(),
		entry
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn package_identity_requires_matching_json_strings() {
	let (f, url, schema) = setup().await;
	let mut entry = model();
	entry.id = "1".into();
	entry.kind = "skill".into();
	entry.config = json!({"instructions":"Complete the task"});
	let package = aidash::registry::Package {
		entity: entry,
		author: "Fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let record = f.registry.publish(&f.store.pool, package).await.unwrap();
	for (field, invalid) in [
		("id", json!(1)),
		("id", json!(true)),
		("id", Value::Null),
		("id", json!(["1"])),
		("id", json!({})),
		("id", json!("other")),
		("version", json!(1)),
		("version", Value::Null),
		("version", json!("2.0.0")),
	] {
		let mut manifest = record.manifest.clone();
		manifest["entity"][field] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	for field in ["id", "version"] {
		let mut manifest = record.manifest.clone();
		manifest["entity"].as_object_mut().unwrap().remove(field);
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	for malformed in [
		json!({"entity":{"id":"1","version":"1.0.0","kind":"skill","name":{"en":"Skill"},"description":{"en":"Description"},"config":{"instructions":"Complete the task"}},"author":"Fixture","permissions":[7],"dependencies":[]}),
		json!({"entity":{"id":"1","version":"1.0.0","kind":"skill","name":{"en":"Skill"},"description":{"en":"Description"},"config":{"instructions":"Complete the task"},"unexpected":true},"author":"Fixture","permissions":[],"dependencies":[]}),
		json!({"entity":{"id":"1","version":"1.0.0","kind":"skill","name":{"en":7},"description":{"en":"Description"},"config":{"instructions":"Complete the task"}},"author":"Fixture","permissions":[],"dependencies":[]}),
	] {
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, malformed).await,
			"packages_identity",
		);
	}
	for invalid_schema in [
		json!({"type":7}),
		json!({"items":7}),
		json!({"properties":{"count":{"required":"name"}}}),
	] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["schema"] = invalid_schema;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	check_rejected(
		update_package_digest(
			&f.store.pool,
			&record.id,
			&record.version,
			"sha256:0000000000000000000000000000000000000000000000000000000000000000",
		)
		.await,
		"packages_digest",
	);
	let alternate_source = format!(" \n{}\n", record.manifest);
	let alternate_digest = format!("sha256:{:x}", Sha256::digest(alternate_source.as_bytes()));
	update_package_source(
		&f.store.pool,
		&record.id,
		&record.version,
		&alternate_source,
		&alternate_digest,
	)
	.await
	.unwrap();
	f.registry
		.install(
			&f.store.pool,
			&record.id,
			&record.version,
			&alternate_digest,
			json!({}),
		)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn package_digest_backfill_preserves_previous_publish_serialization() {
	use migration::MigratorTrait;
	let (f, url, schema) = setup().await;
	let mut entity = model();
	entity.id = "legacy-digest".into();
	entity.kind = "skill".into();
	entity.schema = json!({
		"type":"object",
		"properties":{"fraction":{"type":"number","minimum":1.25e-20,"maximum":std::f64::consts::PI}},
	});
	entity.config = json!({
		"instructions":"Preserve this installed package",
		"future_extension":{"small":1.25e-20,"large":1.0e20,"precise":f64::from_bits(0x3fb999999999999b)},
	});
	let expected_entity = entity.clone();
	let record = f
		.registry
		.publish(
			&f.store.pool,
			aidash::registry::Package {
				entity,
				author: "Fixture".into(),
				permissions: vec![],
				dependencies: vec![],
			},
		)
		.await
		.unwrap();
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	// Remove only this migration's new source column, leaving a package row and
	// digest produced by Registry.publish exactly as on the old schema.
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	let installed = f
		.registry
		.install(
			&f.store.pool,
			&record.id,
			&record.version,
			&record.digest,
			json!({}),
		)
		.await
		.unwrap();
	assert_eq!(installed, expected_entity);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn package_agent_config_requires_all_typed_fields() {
	let (f, url, schema) = setup().await;
	let agent: Entry = serde_json::from_value(json!({
		"id":"packaged-agent",
		"version":"1.0.0",
		"kind":"agent",
		"name":{"en":"Packaged agent"},
		"description":{"en":"Fixture"},
		"capabilities":[],
		"tags":[],
		"languages":[],
		"skills":[],
		"schema":{},
		"config":{
			"model":{"id":"test-model","version":"1.0.0"},
			"instructions":"Work",
			"tools":[],
			"skills":[],
			"cluster":null,
			"max_steps":64
		}
	}))
	.unwrap();
	let record = f
		.registry
		.publish(
			&f.store.pool,
			aidash::registry::Package {
				entity: agent,
				author: "Fixture".into(),
				permissions: vec![],
				dependencies: vec![],
			},
		)
		.await
		.unwrap();
	for invalid in [json!("64"), json!(0), json!(1001), json!([])] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["config"]["max_steps"] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	for invalid in [json!(7), json!(""), json!(" \t\n\u{2003}")] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["config"]["instructions"] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	for invalid in [
		json!(7),
		json!({"id":"cluster"}),
		json!({"id":7,"version":"1.0.0"}),
	] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["config"]["cluster"] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	update_package_manifest(&f.store.pool, &record.id, &record.version, record.manifest)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn package_tool_config_uses_registry_validation() {
	let (f, url, schema) = setup().await;
	let mut tool = model();
	tool.id = "packaged-tool".into();
	tool.kind = "tool".into();
	tool.config = json!({"transport":"native","operation":"echo"});
	let record = f
		.registry
		.publish(
			&f.store.pool,
			aidash::registry::Package {
				entity: tool,
				author: "Fixture".into(),
				permissions: vec![],
				dependencies: vec![],
			},
		)
		.await
		.unwrap();
	for invalid in [
		json!({"transport":"bogus"}),
		json!({"transport":"native","operation":7}),
		json!({"transport":"native","operation":"http_get"}),
	] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["config"] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	for invalid in [json!(" "), json!("\t\n\u{2003}"), json!("")] {
		let mut manifest = record.manifest.clone();
		manifest["entity"]["config"]["instructions"] = invalid;
		check_rejected(
			update_package_manifest(&f.store.pool, &record.id, &record.version, manifest).await,
			"packages_identity",
		);
	}
	cleanup(f, &url, &schema).await;
}

async fn insert_values(
	pool: &sqlx::PgPool,
	table: &str,
	values: &[(&str, SimpleExpr)],
) -> Result<(), sqlx::Error> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new(table))
			.columns(values.iter().map(|(name, _)| Alias::new(*name)))
			.values_panic(values.iter().map(|(_, value)| value.clone()))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.map(|_| ())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn registry_json_shapes_remain_deserializable() {
	let (f, url, schema) = setup().await;
	let good = serde_json::to_value(model()).unwrap();
	for invalid_schema in [
		json!({"type":7}),
		json!({"items":7}),
		json!({"properties":{"count":{"required":"name"}}}),
	] {
		let mut invalid = good.clone();
		invalid["schema"] = invalid_schema;
		check_rejected(
			insert_entry(&f.store.pool, &invalid).await,
			"registry_metadata_shape",
		);
	}
	for field in ["capabilities", "tags", "languages", "skills"] {
		for invalid in [
			json!(7),
			Value::Null,
			json!(true),
			json!({}),
			json!([]),
			json!(["nested"]),
		] {
			let mut entry = good.clone();
			entry[field] = json!(["valid", invalid]);
			check_rejected(
				insert_entry(&f.store.pool, &entry).await,
				"registry_metadata_shape",
			);
		}
	}
	let mut unknown = good.clone();
	unknown["unexpected"] = json!(true);
	check_rejected(
		insert_entry(&f.store.pool, &unknown).await,
		"registry_metadata_shape",
	);
	for field in ["capabilities", "tags", "languages", "skills"] {
		unknown.as_object_mut().unwrap().remove(field);
	}
	unknown.as_object_mut().unwrap().remove("unexpected");
	insert_entry(&f.store.pool, &unknown).await.unwrap();
	assert_eq!(f.registry.list(&Default::default()).await.unwrap().len(), 1);
	let mut full = good;
	full["id"] = json!("full");
	for field in ["capabilities", "tags", "languages", "skills"] {
		full[field] = json!(["one", "two"]);
	}
	insert_entry(&f.store.pool, &full).await.unwrap();
	assert_eq!(f.registry.list(&Default::default()).await.unwrap().len(), 2);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn model_and_agent_configs_reject_unusable_shapes() {
	let (f, url, schema) = setup().await;
	let good = serde_json::to_value(model()).unwrap();
	for (field, value) in [
		("unexpected", json!(true)),
		("credential_env", json!(7)),
		("modalities", json!(["text", 7])),
		("modalities", json!(["text", ["audio"]])),
		("context_window", json!(1e30)),
	] {
		let mut entry = good.clone();
		entry["config"][field] = value;
		check_rejected(
			insert_entry(&f.store.pool, &entry).await,
			"registry_model_config",
		);
	}
	let mut missing = good.clone();
	missing["config"].as_object_mut().unwrap().remove("cost");
	check_rejected(
		insert_entry(&f.store.pool, &missing).await,
		"registry_model_config",
	);
	let mut agent = good.clone();
	agent["id"] = json!("agent");
	agent["kind"] = json!("agent");
	agent["config"] = json!({"instructions":"Work"});
	check_rejected(
		insert_entry(&f.store.pool, &agent).await,
		"registry_agent_config",
	);
	insert_entry(&f.store.pool, &good).await.unwrap();
	let mut other_kind = serde_json::to_value(model()).unwrap();
	other_kind["id"] = json!("not-a-model");
	other_kind["kind"] = json!("tool");
	other_kind["config"] = json!({"transport":"native","operation":"echo"});
	insert_entry(&f.store.pool, &other_kind).await.unwrap();
	let mut skill = serde_json::to_value(model()).unwrap();
	skill["id"] = json!("test-skill");
	skill["kind"] = json!("skill");
	skill["config"] = json!({"instructions":"Help with a focused task"});
	insert_entry(&f.store.pool, &skill).await.unwrap();
	for invalid in [
		Value::Null,
		json!([]),
		json!({}),
		json!({"id":"m"}),
		json!({"id":7,"version":"1.0.0"}),
		json!({"id":"m","version":7}),
	] {
		agent["config"]["model"] = invalid;
		check_rejected(
			insert_entry(&f.store.pool, &agent).await,
			"registry_agent_config",
		);
	}
	agent["config"]["model"] = json!({"id":"test-model","version":"1.0.0"});
	let mut missing_model = agent.clone();
	missing_model["id"] = json!("missing-model-agent");
	missing_model["config"]["model"] = json!({"id":"does-not-exist","version":"1.0.0"});
	check_foreign_key_rejected(
		insert_entry(&f.store.pool, &missing_model).await,
		"registry_agent_model_target",
	);
	let mut wrong_kind_model = agent.clone();
	wrong_kind_model["id"] = json!("wrong-kind-model-agent");
	wrong_kind_model["config"]["model"] = json!({"id":"not-a-model","version":"1.0.0"});
	check_foreign_key_rejected(
		insert_entry(&f.store.pool, &wrong_kind_model).await,
		"registry_agent_model_target",
	);
	for (field, invalid) in [
		("tools", json!([7])),
		("skills", json!([{"id":"skill"}])),
		("cluster", json!(7)),
		("unexpected", json!(true)),
	] {
		let mut malformed = agent.clone();
		malformed["config"][field] = invalid;
		check_rejected(
			insert_entry(&f.store.pool, &malformed).await,
			"registry_agent_config",
		);
	}
	let mut fractional_steps = agent.clone();
	fractional_steps["config"]["max_steps"] = json!(1.0);
	check_rejected(
		insert_entry(&f.store.pool, &fractional_steps).await,
		"registry_agent_config",
	);
	let mut integer_steps = agent.clone();
	integer_steps["id"] = json!("integer-agent");
	integer_steps["config"]["max_steps"] = json!(1);
	let _: aidash::registry::AgentConfig =
		serde_json::from_value(integer_steps["config"].clone()).unwrap();
	insert_entry(&f.store.pool, &integer_steps).await.unwrap();
	let _: aidash::registry::AgentConfig = serde_json::from_value(agent["config"].clone()).unwrap();
	insert_entry(&f.store.pool, &agent).await.unwrap();
	let mut cluster = serde_json::to_value(model()).unwrap();
	cluster["id"] = json!("test-cluster");
	cluster["kind"] = json!("cluster");
	cluster["config"] = json!({"coordinator":{"id":"agent","version":"1.0.0"}});
	insert_entry(&f.store.pool, &cluster).await.unwrap();
	let mut linked_agent_config = agent["config"].clone();
	linked_agent_config["tools"] = json!([{"id":"not-a-model","version":"1.0.0"}]);
	linked_agent_config["skills"] = json!([{"id":"test-skill","version":"1.0.0"}]);
	linked_agent_config["cluster"] = json!({"id":"test-cluster","version":"1.0.0"});
	update_registry_config(&f.store.pool, "agent", "1.0.0", linked_agent_config)
		.await
		.unwrap();
	for (id, field, target) in [
		(
			"missing-tool",
			"tools",
			json!({"id":"absent","version":"1.0.0"}),
		),
		(
			"wrong-kind-tool",
			"tools",
			json!({"id":"test-model","version":"1.0.0"}),
		),
		(
			"wrong-kind-skill",
			"skills",
			json!({"id":"not-a-model","version":"1.0.0"}),
		),
		(
			"missing-skill",
			"skills",
			json!({"id":"absent","version":"1.0.0"}),
		),
		(
			"wrong-kind-cluster",
			"cluster",
			json!({"id":"test-skill","version":"1.0.0"}),
		),
		(
			"missing-cluster",
			"cluster",
			json!({"id":"absent","version":"1.0.0"}),
		),
	] {
		let mut invalid_reference = agent.clone();
		invalid_reference["id"] = json!(id);
		if field == "cluster" {
			invalid_reference["config"][field] = target;
		} else {
			invalid_reference["config"][field] = json!([target]);
		}
		check_foreign_key_rejected(
			insert_entry(&f.store.pool, &invalid_reference).await,
			"registry_agent_resource_target",
		);
	}
	let truncate_model_refs = sqlx::query("TRUNCATE registry_agent_model_refs")
		.execute(&f.store.pool)
		.await
		.map(|_| ());
	check_rejected(truncate_model_refs, "registry_agent_model_reference");
	let truncate_resource_refs = sqlx::query("TRUNCATE registry_agent_resource_refs")
		.execute(&f.store.pool)
		.await
		.map(|_| ());
	check_rejected(truncate_resource_refs, "registry_agent_resource_reference");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn tool_configs_reject_undecodable_shapes() {
	let (f, url, schema) = setup().await;
	let mut tool = serde_json::to_value(model()).unwrap();
	tool["id"] = json!("tool");
	tool["kind"] = json!("tool");
	for config in [
		json!({"transport":"http"}),
		json!({"transport":"http","endpoint":"not a URL","replay":"read_only"}),
		json!({"transport":"http","endpoint":"ftp://example.com","replay":"read_only"}),
		json!({"transport":"http","endpoint":"http:///missing-host","replay":"read_only"}),
		json!({"transport":"http","endpoint":"https://user:pass@example.com","replay":"read_only"}),
		json!({"transport":"http","endpoint":"http://example.com?token=value","replay":"read_only"}),
		json!({"transport":"http","endpoint":"https://example.com/path#fragment","replay":"read_only"}),
		json!({"transport":"mcp","endpoint":"http://example.com?token=value","tool_name":"call","replay":"read_only"}),
		json!({"transport":"native","operation":"http_get","allowed_hosts":[]}),
		json!({"transport":"native","operation":"echo","allowed_hosts":[7]}),
		json!({"transport":"http","endpoint":"http://localhost","replay":"invalid"}),
		json!({"transport":"mcp","endpoint":"http://localhost","tool_name":"call","replay":"idempotent"}),
		json!({"transport":"mcp","endpoint":"http://localhost","tool_name":"call","replay":"idempotent","idempotency_argument":" \t\n\u{2003}"}),
		json!({"transport":"agent","node_id":"bad node","agent":{"id":"agent","version":"1.0.0"}}),
		json!({"transport":"http","endpoint":"http://localhost","replay":"read_only","unexpected":true}),
		json!({"transport":"http","endpoint":"http://localhost","credential_env":"OPENAI_API_KEY","replay":"read_only"}),
		json!({"transport":"mcp","endpoint":"http://localhost","credential_env":"SECRET_TOKEN","tool_name":"call","replay":"read_only"}),
	] {
		tool["config"] = config;
		check_rejected(
			insert_entry(&f.store.pool, &tool).await,
			"registry_tool_config",
		);
	}
	for (id, endpoint) in [
		("tool", "http://model_gateway:8080/v1"),
		("tool-leading-underscore", "http://_model_gateway:8080/v1"),
		("tool-underscore-label", "http://model._gateway:8080/v1"),
	] {
		reqwest::Url::parse(endpoint).unwrap();
		tool["id"] = json!(id);
		tool["config"] = json!({
			"transport":"http",
			"endpoint":endpoint,
			"credential_env":null,
			"replay":"read_only"
		});
		insert_entry(&f.store.pool, &tool).await.unwrap();
	}
	tool["id"] = json!("secret-tool");
	tool["config"] = json!({
		"transport":"http",
		"endpoint":"http://localhost",
		"credential_env":"AIDASH_SECRET_TOOL_TOKEN",
		"replay":"read_only"
	});
	insert_entry(&f.store.pool, &tool).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn compactor_and_embedding_configs_reject_undecodable_shapes() {
	let (f, url, schema) = setup().await;
	let mut entry = serde_json::to_value(model()).unwrap();
	entry["id"] = json!("compactor");
	entry["kind"] = json!("compactor");
	for invalid in [
		json!({}),
		json!({"provider":"wrong"}),
		json!({"provider":"typesafe-system-one","endpoint":"http://localhost","model":"m","credential_env":"AIDASH_SECRET_X","max_request_bytes":1024,"max_questions":1,"max_response_bytes":"128"}),
		json!({"provider":"typesafe-system-one","endpoint":"http://localhost","model":"m","credential_env":"AIDASH_SECRET_X","max_request_bytes":1024,"max_questions":1,"max_response_bytes":128,"unexpected":true}),
	] {
		entry["config"] = invalid;
		check_rejected(
			insert_entry(&f.store.pool, &entry).await,
			"registry_compactor_config",
		);
	}
	entry["config"] = json!({
		"provider":"typesafe-system-one",
		"endpoint":"http://localhost:9999/v1",
		"model":"jev-latest",
		"credential_env":"AIDASH_SECRET_JEV",
		"max_request_bytes":65536,
		"max_questions":8,
		"max_response_bytes":16384
	});
	insert_entry(&f.store.pool, &entry).await.unwrap();

	entry["id"] = json!("embedding");
	entry["kind"] = json!("embedding");
	for invalid in [
		json!({}),
		json!({"provider":"openai","endpoint":"http://localhost","credential_env":null,"model":"m","model_version":"1","dimensions":0}),
		json!({"provider":"openai","endpoint":"http://localhost","credential_env":7,"model":"m","model_version":"1","dimensions":3}),
		json!({"provider":"openai","endpoint":"http://localhost","credential_env":null,"model":"m","model_version":"1","dimensions":3,"unexpected":true}),
	] {
		entry["config"] = invalid;
		check_rejected(
			insert_entry(&f.store.pool, &entry).await,
			"registry_embedding_config",
		);
	}
	entry["config"] = json!({
		"provider":"openai",
		"endpoint":"http://localhost:9999/v1",
		"credential_env":null,
		"model":"embedding-model",
		"model_version":"1",
		"dimensions":1536
	});
	insert_entry(&f.store.pool, &entry).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn requirements_and_run_state_reject_wrong_shapes() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Task".into(),
				description: "Work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let claimed_without_owner = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("status"), Expr::val("CLAIMED"))
		.value(Alias::new("owner"), Expr::cust("NULL"))
		.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(task.id)))
		.to_string(PostgresQueryBuilder);
	check_rejected(
		sqlx::query(&claimed_without_owner)
			.execute(&f.store.pool)
			.await
			.map(|_| ()),
		"tasks_active_owner",
	);
	for invalid in [
		json!({"unexpected":true}),
		json!({"capability":7}),
		json!({"query":["text"]}),
		json!({"model":{}}),
	] {
		check_rejected(
			update(&f.store.pool, "tasks", "requirements", Expr::val(invalid)).await,
			"tasks_content",
		);
	}
	let valid = json!({"kind":null,"query":"text","capability":"code","language":"en","skill":"read","tag":"tag","model":"model"});
	let _: aidash::registry::Search = serde_json::from_value(valid.clone()).unwrap();
	update(&f.store.pool, "tasks", "requirements", Expr::val(valid))
		.await
		.unwrap();
	let run = f
		.store
		.accept_run(&task, "aidash://remote", "executor", "1.0.0")
		.await
		.unwrap();
	for column in ["context", "pending"] {
		for invalid in [Value::Null, json!([]), json!(7), json!("text")] {
			check_rejected(
				update(&f.store.pool, "runs", column, Expr::val(invalid)).await,
				"runs_counters",
			);
		}
		update(&f.store.pool, "runs", column, Expr::val(json!({})))
			.await
			.unwrap();
	}
	for malformed in [
		json!({"retry_at":"not a timestamp"}),
		json!({"retry_at":null}),
		json!({"wake_at":"not a timestamp"}),
		json!({"wake_at":[]}),
	] {
		check_rejected(
			update(&f.store.pool, "runs", "pending", Expr::val(malformed)).await,
			"runs_counters",
		);
	}
	update(
		&f.store.pool,
		"runs",
		"pending",
		Expr::val(json!({
			"retry_at":"2030-01-01T00:00:00Z",
			"wake_at":"2030-01-01T00:00:00+00:00"
		})),
	)
	.await
	.unwrap();
	update_run_state(
		&f.store.pool,
		run.id,
		"WAITING",
		json!({"wake_at":"2030-01-01T00:00:00Z"}),
	)
	.await
	.unwrap();
	update_run_state(&f.store.pool, run.id, "READY", json!({}))
		.await
		.unwrap();
	check_rejected(
		update_run_state(
			&f.store.pool,
			run.id,
			"WAITING",
			json!({"human_request_id":"not-a-uuid"}),
		)
		.await,
		"runs_waiting_request",
	);
	check_foreign_key_rejected(
		update_run_state(
			&f.store.pool,
			run.id,
			"WAITING",
			json!({"human_request_id":uuid::Uuid::new_v4()}),
		)
		.await,
		"runs_human_request_ref",
	);
	let request = f
		.store
		.human_request(&run, "QUESTION", "Continue?", "waiting-shape-test")
		.await
		.unwrap();
	update_run_state(
		&f.store.pool,
		run.id,
		"WAITING",
		json!({"human_request_id":request.id}),
	)
	.await
	.unwrap();
	check_foreign_key_rejected(
		update(
			&f.store.pool,
			"human_requests",
			"id",
			uuid_expr(uuid::Uuid::new_v4()),
		)
		.await,
		"runs_human_request_ref",
	);
	let other_task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Other request task".into(),
				description: "Work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let other_run = f
		.store
		.accept_run(&other_task, "aidash://remote", "executor", "1.0.0")
		.await
		.unwrap();
	check_foreign_key_rejected(
		update(
			&f.store.pool,
			"human_requests",
			"run_id",
			uuid_expr(other_run.id),
		)
		.await,
		"runs_human_request_ref",
	);
	let delete_request = Query::delete()
		.from_table(Alias::new("human_requests"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	check_foreign_key_rejected(
		sqlx::query(&delete_request)
			.bind(request.id)
			.execute(&f.store.pool)
			.await
			.map(|_| ()),
		"runs_human_request_ref",
	);
	update_run_state(&f.store.pool, other_run.id, "COMPLETED", json!({}))
		.await
		.unwrap();
	update_run_state(&f.store.pool, run.id, "READY", json!({}))
		.await
		.unwrap();
	let tool_response = json!({
		"text":"",
		"tool_calls":[{"id":"call-1","name":"echo","arguments":{}}],
		"input_tokens":0,
		"output_tokens":0
	});
	let valid_tool_pending = json!({"response":tool_response,"cursor":1});
	update_run_state(
		&f.store.pool,
		run.id,
		"TOOL_CALL",
		valid_tool_pending.clone(),
	)
	.await
	.unwrap();
	update_run_state(&f.store.pool, run.id, "READY", json!({}))
		.await
		.unwrap();
	for invalid in [
		json!({"response":{"text":"x","tool_calls":"bad","input_tokens":0,"output_tokens":0},"cursor":0}),
		json!({"response":tool_response,"cursor":2}),
		json!({"response":{"text":"x","tool_calls":[{"id":7,"name":"echo","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0}),
		json!({"response":{"tool_calls":[],"input_tokens":0,"output_tokens":0},"cursor":0}),
		json!({"response":{"text":"x","tool_calls":[],"output_tokens":0},"cursor":0}),
		json!({"response":{"text":"x","tool_calls":[{"name":"echo","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0}),
		json!({"response":{"text":"x","tool_calls":[{"id":"call-1","arguments":{}}],"input_tokens":0,"output_tokens":0},"cursor":0}),
	] {
		check_rejected(
			update_run_state(&f.store.pool, run.id, "TOOL_CALL", invalid).await,
			"runs_counters",
		);
	}
	let waiting_tool_pending = json!({
		"resume_phase":"TOOL_CALL",
		"wake_at":"2030-01-01T00:00:00Z",
		"response":tool_response,
		"cursor":1,
	});
	update_run_state(
		&f.store.pool,
		run.id,
		"WAITING",
		waiting_tool_pending.clone(),
	)
	.await
	.unwrap();
	let mut invalid_waiting_tool = waiting_tool_pending;
	invalid_waiting_tool["cursor"] = json!(2);
	check_rejected(
		update_run_state(&f.store.pool, run.id, "WAITING", invalid_waiting_tool).await,
		"runs_counters",
	);
	update_run_state(&f.store.pool, run.id, "READY", json!({}))
		.await
		.unwrap();
	check_rejected(
		update(&f.store.pool, "runs", "revision", Expr::val(i64::MAX)).await,
		"runs_counters",
	);
	let second_task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Second task".into(),
				description: "Work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let second_run = f
		.store
		.accept_run(&second_task, "aidash://remote", "executor", "1.0.0")
		.await
		.unwrap();
	let max_safe_revision = Query::update()
		.table(Alias::new("runs"))
		.value(Alias::new("revision"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&max_safe_revision)
		.bind(i64::MAX - 1)
		.bind(run.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&max_safe_revision)
		.bind(i64::MAX - 2)
		.bind(second_run.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let third_task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Third task".into(),
				description: "Work".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let third_run = f
		.store
		.accept_run(&third_task, "aidash://remote", "executor", "1.0.0")
		.await
		.unwrap();
	let leased = f
		.store
		.lease_run(uuid::Uuid::new_v4(), 30)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(leased.id, third_run.id);
	for malformed in [
		json!({"summary":7}),
		json!({"history":7}),
		json!({"usage":7}),
		json!({"usage":{"input_tokens":0}}),
		json!({"usage":{"input_tokens":0,"output_tokens":0,"context_window":4096}}),
		json!({"usage":{"input_tokens":"0","output_tokens":0,"context_window":4096,"compactions":0}}),
		json!({"compactions":-1}),
		json!({"compactions":1.5}),
		json!({"compactions":4294967296_u64}),
	] {
		check_rejected(
			update(&f.store.pool, "runs", "context", Expr::val(malformed)).await,
			"runs_counters",
		);
	}
	update(
		&f.store.pool,
		"runs",
		"context",
		Expr::val(json!({"usage":{},"history":[{"kind":"message"}]})),
	)
	.await
	.unwrap();
	update(
		&f.store.pool,
		"runs",
		"context",
		Expr::val(json!({
			"summary":"",
			"history":[],
			"usage":{"input_tokens":0,"output_tokens":0,"context_window":4096,"compactions":0,"future_metric":true},
			"compactions":0
		})),
	)
	.await
	.unwrap();
	let lease_deadline_update = |deadline: &str| {
		Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("lease_owner"), Expr::cust("gen_random_uuid()"))
			.value(
				Alias::new("lease_until"),
				Expr::cust(format!("'{deadline}'::timestamptz")),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder)
	};
	for deadline in ["infinity", "-infinity"] {
		check_rejected(
			sqlx::query(&lease_deadline_update(deadline))
				.bind(run.id)
				.execute(&f.store.pool)
				.await
				.map(|_| ()),
			"runs_lease",
		);
	}
	sqlx::query(&lease_deadline_update("2030-01-01 00:00:00+00"))
		.bind(run.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

fn index_spec() -> Value {
	json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9999/v1","model":"embedding","model_version":"1","dimensions":3},
        "vector":{"provider":"qdrant","endpoint":"http://localhost:6333"},
        "enabled":true,"auto_context":false,"max_sources":64,"max_results":10,"max_result_tokens":4096,"max_input_bytes":8192})
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn semantic_specs_and_sources_reject_undecodable_records() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let spec = index_spec();
	let _: aidash::semantic::IndexSpec = serde_json::from_value(spec.clone()).unwrap();
	insert_values(
		&f.store.pool,
		"semantic_indexes",
		&[
			("workspace_id", uuid_expr(workspace.id)),
			("tenant", Expr::val("test").into()),
			("revision", Expr::val(1).into()),
			("spec", Expr::val(spec.clone()).into()),
			("collection", Expr::val("test").into()),
		],
	)
	.await
	.unwrap();
	check_rejected(
		update(
			&f.store.pool,
			"semantic_indexes",
			"revision",
			Expr::val(i64::MAX),
		)
		.await,
		"semantic_indexes_revision",
	);
	let mut empty_port = spec.clone();
	empty_port["embedding"]["endpoint"] = json!("http://localhost:");
	let runtime: aidash::semantic::IndexSpec = serde_json::from_value(empty_port.clone()).unwrap();
	runtime.validate().unwrap();
	update(
		&f.store.pool,
		"semantic_indexes",
		"spec",
		Expr::val(empty_port),
	)
	.await
	.unwrap();
	for (field, minimum, maximum) in [
		("max_sources", 1, 1024),
		("max_results", 1, 20),
		("max_result_tokens", 128, 32768),
		("max_input_bytes", 128, 32768),
	] {
		for value in [minimum, maximum] {
			let mut boundary = spec.clone();
			boundary[field] = json!(value);
			let runtime: aidash::semantic::IndexSpec =
				serde_json::from_value(boundary.clone()).unwrap();
			runtime.validate().unwrap();
			update(
				&f.store.pool,
				"semantic_indexes",
				"spec",
				Expr::val(boundary),
			)
			.await
			.unwrap();
		}
	}
	for value in [1, 8192] {
		let mut boundary = spec.clone();
		boundary["embedding"]["dimensions"] = json!(value);
		let runtime: aidash::semantic::IndexSpec =
			serde_json::from_value(boundary.clone()).unwrap();
		runtime.validate().unwrap();
		update(
			&f.store.pool,
			"semantic_indexes",
			"spec",
			Expr::val(boundary),
		)
		.await
		.unwrap();
	}
	update(
		&f.store.pool,
		"semantic_indexes",
		"spec",
		Expr::val(spec.clone()),
	)
	.await
	.unwrap();
	let mut invalid_specs = vec![json!({}), json!([]), Value::Null];
	for field in spec.as_object().unwrap().keys() {
		let mut missing = spec.clone();
		missing.as_object_mut().unwrap().remove(field);
		invalid_specs.push(missing);
		let mut wrong = spec.clone();
		wrong[field] = Value::Null;
		invalid_specs.push(wrong);
	}
	for (path, value) in [
		("/unexpected", json!(true)),
		("/max_results", json!(1.5)),
		("/max_sources", json!(-1)),
		("/max_input_bytes", json!("8192")),
		("/enabled", json!("true")),
		("/embedding", json!({})),
		("/vector", json!({})),
	] {
		let mut wrong = spec.clone();
		wrong[path.trim_start_matches('/')] = value;
		invalid_specs.push(wrong);
	}
	for (field, invalid) in [
		("max_sources", [0, 1025]),
		("max_results", [0, 21]),
		("max_result_tokens", [127, 32769]),
		("max_input_bytes", [127, 32769]),
	] {
		for value in invalid {
			let mut wrong = spec.clone();
			wrong[field] = json!(value);
			invalid_specs.push(wrong);
		}
	}
	for value in [0, 8193] {
		let mut wrong = spec.clone();
		wrong["embedding"]["dimensions"] = json!(value);
		invalid_specs.push(wrong);
	}
	for section in ["embedding", "vector"] {
		for field in spec[section].as_object().unwrap().keys() {
			let mut wrong = spec.clone();
			wrong[section].as_object_mut().unwrap().remove(field);
			invalid_specs.push(wrong);
			let mut wrong = spec.clone();
			wrong[section][field] = Value::Null;
			invalid_specs.push(wrong);
		}
		for (field, value) in [("unexpected", json!(true)), ("credential_env", json!(7))] {
			let mut wrong = spec.clone();
			wrong[section][field] = value;
			invalid_specs.push(wrong);
		}
	}
	for (section, field, value) in [
		("embedding", "provider", json!("unsupported")),
		("vector", "provider", json!("pgvector")),
		(
			"embedding",
			"endpoint",
			json!("http://user:password@localhost/v1"),
		),
		(
			"embedding",
			"endpoint",
			json!("http://localhost/v1?token=x"),
		),
		("vector", "endpoint", json!("file:///tmp/vectors")),
		(
			"vector",
			"endpoint",
			json!("http://999.999.999.999/vectors"),
		),
		("embedding", "credential_env", json!("OPENAI_API_KEY")),
		("embedding", "model", json!("  ")),
		("embedding", "model", json!("m".repeat(257))),
		("embedding", "model_version", json!("v".repeat(129))),
	] {
		let mut wrong = spec.clone();
		wrong[section][field] = value;
		invalid_specs.push(wrong);
	}
	for invalid in invalid_specs {
		check_rejected(
			update(
				&f.store.pool,
				"semantic_indexes",
				"spec",
				Expr::val(invalid),
			)
			.await,
			"semantic_indexes_revision",
		);
	}
	insert_values(
		&f.store.pool,
		"semantic_entries",
		&[
			("id", uuid_expr(uuid::Uuid::new_v4())),
			("workspace_id", uuid_expr(workspace.id)),
			("key", Expr::val("entry").into()),
			(
				"source",
				Expr::val(json!({"kind":"memory","text":"text"})).into(),
			),
			("metadata", Expr::val(json!({})).into()),
			("revision", Expr::val(1).into()),
			("point_id", uuid_expr(uuid::Uuid::new_v4())),
			("index_revision", Expr::val(1).into()),
			("state", Expr::val("PENDING").into()),
			("created_by", Expr::val("human").into()),
			(
				"authority",
				Expr::val(json!({
					"credential": null,
					"tenant": "",
					"subject": "operator",
					"subjects": [],
				}))
				.into(),
			),
		],
	)
	.await
	.unwrap();
	for invalid in [
		json!({}),
		json!([]),
		json!({"credential":null,"tenant":"","subject":"operator","subjects":[7]}),
		json!({"credential":"not-a-uuid","tenant":"","subject":"operator","subjects":[]}),
		json!({"credential":null,"tenant":7,"subject":"operator","subjects":[]}),
	] {
		check_rejected(
			update(
				&f.store.pool,
				"semantic_entries",
				"authority",
				Expr::val(invalid),
			)
			.await,
			"semantic_entries_authority",
		);
	}
	for invalid in [
		json!({}),
		json!([]),
		Value::Null,
		json!({"kind":"unknown"}),
		json!({"kind":"memory"}),
		json!({"kind":"memory","text":7}),
		json!({"kind":"memory","text":" \t\u{2003}"}),
		json!({"kind":"memory","text":"text","unexpected":true}),
		json!({"kind":"artifact","id":"invalid"}),
		json!({"kind":"message","id":7}),
		json!({"kind":"artifact"}),
		json!({"kind":"message","id":uuid::Uuid::new_v4(),"text":"extra"}),
	] {
		check_rejected(
			update(
				&f.store.pool,
				"semantic_entries",
				"source",
				Expr::val(invalid),
			)
			.await,
			"semantic_entries_counters",
		);
	}
	update(
		&f.store.pool,
		"semantic_entries",
		"deleted",
		Expr::val(true),
	)
	.await
	.unwrap();
	update(
		&f.store.pool,
		"semantic_entries",
		"source",
		Expr::val(json!({"kind":"memory","text":""})),
	)
	.await
	.unwrap();
	update(
		&f.store.pool,
		"semantic_entries",
		"source",
		Expr::val(json!({"kind":"memory","text":"text"})),
	)
	.await
	.unwrap();
	update(
		&f.store.pool,
		"semantic_entries",
		"deleted",
		Expr::val(false),
	)
	.await
	.unwrap();
	let id = uuid::Uuid::new_v4();
	for kind in ["artifact", "message"] {
		for id in [
			id.to_string(),
			id.simple().to_string(),
			id.urn().to_string(),
			id.braced().to_string(),
		] {
			let source = json!({"kind":kind,"id":id});
			let _: aidash::semantic::Source = serde_json::from_value(source.clone()).unwrap();
			update(
				&f.store.pool,
				"semantic_entries",
				"source",
				Expr::val(source),
			)
			.await
			.unwrap();
		}
	}
	update(
		&f.store.pool,
		"semantic_entries",
		"source",
		Expr::val(json!({"kind":"memory","text":"x".repeat(129)})),
	)
	.await
	.unwrap();
	let mut smaller_limit = spec.clone();
	smaller_limit["max_input_bytes"] = json!(128);
	check_rejected(
		update(
			&f.store.pool,
			"semantic_indexes",
			"spec",
			Expr::val(smaller_limit.clone()),
		)
		.await,
		"semantic_index_input_bytes",
	);
	update(
		&f.store.pool,
		"semantic_entries",
		"source",
		Expr::val(json!({"kind":"memory","text":"short"})),
	)
	.await
	.unwrap();
	update(
		&f.store.pool,
		"semantic_indexes",
		"spec",
		Expr::val(smaller_limit),
	)
	.await
	.unwrap();
	check_rejected(
		update(
			&f.store.pool,
			"semantic_entries",
			"source",
			Expr::val(json!({"kind":"memory","text":"é".repeat(65)})),
		)
		.await,
		"semantic_memory_input_bytes",
	);
	cleanup(f, &url, &schema).await;
}

fn dependencies_update() -> String {
	Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("dependencies"), Expr::cust("$2"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder)
}
fn delete_task() -> String {
	Query::delete()
		.from_table(Alias::new("tasks"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder)
}
fn dependency_error(error: sqlx::Error) {
	let db = error.as_database_error().unwrap();
	assert_eq!(db.code().as_deref(), Some("23503"), "{error}");
	assert_eq!(
		db.constraint(),
		Some("tasks_dependencies_target_workspace"),
		"{error}"
	);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn task_dependencies_enforce_existence_ownership_and_reverse_changes() {
	use migration::MigratorTrait;
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
	let target = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let foreign = f
		.store
		.create_task(other.id, &input, "human", None)
		.await
		.unwrap();
	let query = dependencies_update();
	for dependency in [uuid::Uuid::new_v4(), foreign.id] {
		dependency_error(
			sqlx::query(&query)
				.bind(task.id)
				.bind(vec![dependency])
				.execute(&f.store.pool)
				.await
				.unwrap_err(),
		);
	}
	// Bypassing the application on insertion must reject a foreign dependency too.
	dependency_error(
		insert_values(
			&f.store.pool,
			"tasks",
			&[
				("id", uuid_expr(uuid::Uuid::new_v4())),
				("workspace_id", uuid_expr(workspace.id)),
				("title", Expr::val("Direct").into()),
				("created_by", Expr::val("human").into()),
				("description", Expr::val("Work").into()),
				(
					"dependencies",
					Expr::cust(format!("ARRAY['{}'::uuid]", foreign.id)),
				),
			],
		)
		.await
		.unwrap_err(),
	);
	// Duplicate array entries are compatible; the projection stores distinct edges.
	sqlx::query(&query)
		.bind(task.id)
		.bind(vec![target.id, target.id])
		.execute(&f.store.pool)
		.await
		.unwrap();
	dependency_error(
		sqlx::query(&delete_task())
			.bind(target.id)
			.execute(&f.store.pool)
			.await
			.unwrap_err(),
	);
	let move_target = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("workspace_id"), uuid_expr(other.id))
		.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(target.id)))
		.to_string(PostgresQueryBuilder);
	dependency_error(
		sqlx::query(&move_target)
			.execute(&f.store.pool)
			.await
			.unwrap_err(),
	);
	check_rejected(
		sqlx::query(
			&Query::delete()
				.from_table(Alias::new("task_dependencies"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
		.map(|_| ()),
		"tasks_dependencies_managed",
	);
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	dependency_error(
		sqlx::query(&delete_task())
			.bind(target.id)
			.execute(&f.store.pool)
			.await
			.unwrap_err(),
	);
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	sqlx::query(&query)
		.bind(task.id)
		.bind(vec![foreign.id])
		.execute(&f.store.pool)
		.await
		.unwrap();
	assert!(migration::Migrator::up(&db, None).await.is_err());
	// Correct invalid history and retry the transactionally rolled-back migration.
	sqlx::query(&query)
		.bind(task.id)
		.bind(vec![target.id])
		.execute(&f.store.pool)
		.await
		.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	sqlx::query(&delete_task())
		.bind(task.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&delete_task())
		.bind(target.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn concurrent_dependency_changes_cannot_race_target_deletion() {
	use std::time::Duration;
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
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
	for deletion_first in [true, false] {
		let target = f
			.store
			.create_task(workspace.id, &input, "human", None)
			.await
			.unwrap();
		let mut transaction = f.store.pool.begin().await.unwrap();
		if deletion_first {
			sqlx::query(&delete_task())
				.bind(target.id)
				.execute(&mut *transaction)
				.await
				.unwrap();
		} else {
			sqlx::query(&dependencies_update())
				.bind(task.id)
				.bind(vec![target.id])
				.execute(&mut *transaction)
				.await
				.unwrap();
		}
		let pool = f.store.pool.clone();
		let mut competing = tokio::spawn(async move {
			if deletion_first {
				sqlx::query(&dependencies_update())
					.bind(task.id)
					.bind(vec![target.id])
					.execute(&pool)
					.await
			} else {
				sqlx::query(&delete_task())
					.bind(target.id)
					.execute(&pool)
					.await
			}
		});
		assert!(
			tokio::time::timeout(Duration::from_millis(100), &mut competing)
				.await
				.is_err()
		);
		transaction.commit().await.unwrap();
		dependency_error(
			tokio::time::timeout(Duration::from_secs(5), competing)
				.await
				.unwrap()
				.unwrap()
				.unwrap_err(),
		);
	}
	cleanup(f, &url, &schema).await;
}

fn uuid_expr(id: uuid::Uuid) -> SimpleExpr {
	Expr::cust(format!("'{id}'::uuid"))
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn task_parent_cycle_guard_rejects_direct_cycles() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let parent = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let child = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let parent_update = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&parent_update)
		.bind(child.id)
		.bind(parent.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let error = sqlx::query(&parent_update)
		.bind(parent.id)
		.bind(child.id)
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some("tasks_parent_cycle"), "{error}");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn task_dependency_cycles_include_parent_edges() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let first = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let second = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let set_dependencies = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("dependencies"), Expr::cust("ARRAY[$1::uuid]"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&set_dependencies)
		.bind(first.id)
		.bind(second.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let error = sqlx::query(&set_dependencies)
		.bind(second.id)
		.bind(first.id)
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(
		database.constraint(),
		Some("tasks_dependency_cycle"),
		"{error}"
	);

	let set_parent = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&set_parent)
		.bind(first.id)
		.bind(second.id)
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(
		database.constraint(),
		Some("tasks_dependency_cycle"),
		"{error}"
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn task_cycle_checks_deduplicate_diamond_reachability() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let mut levels: Vec<Vec<uuid::Uuid>> = Vec::new();
	for _ in 0..20 {
		let mut level = Vec::new();
		for _ in 0..2 {
			level.push(
				f.store
					.create_task(workspace.id, &input, "human", None)
					.await
					.unwrap()
					.id,
			);
		}
		if let Some(previous) = levels.last() {
			for task_id in &level {
				sqlx::query(&dependencies_update())
					.bind(task_id)
					.bind(previous.clone())
					.execute(&f.store.pool)
					.await
					.unwrap();
			}
		}
		levels.push(level);
	}
	let error = sqlx::query(&dependencies_update())
		.bind(levels[0][0])
		.bind(vec![levels.last().unwrap()[0]])
		.execute(&f.store.pool)
		.await
		.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(
		database.constraint(),
		Some("tasks_dependency_cycle"),
		"{error}"
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn historical_task_cycles_fail_migration_through_query_validation() {
	use migration::MigratorTrait;
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let first = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let second = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	let parent_update = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	sqlx::query(&parent_update)
		.bind(second.id)
		.bind(first.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&parent_update)
		.bind(first.id)
		.bind(second.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let error = migration::Migrator::up(&db, None).await.unwrap_err();
	assert!(error.to_string().contains("tasks_parent_cycle"), "{error}");
	let clear_parents = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("parent_id"), Expr::cust("NULL"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&clear_parents)
		.execute(&f.store.pool)
		.await
		.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();

	// Parent traversal alone is acyclic here; the backfill must catch the
	// child-to-parent dependency opposing the parent's parent-to-child edge.
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	sqlx::query(&parent_update)
		.bind(first.id)
		.bind(second.id)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&dependencies_update())
		.bind(second.id)
		.bind(vec![first.id])
		.execute(&f.store.pool)
		.await
		.unwrap();
	let error = migration::Migrator::up(&db, None).await.unwrap_err();
	assert!(
		error.to_string().contains("tasks_dependency_cycle"),
		"{error}"
	);
	sqlx::query(&clear_parents)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&dependencies_update())
		.bind(second.id)
		.bind(Vec::<uuid::Uuid>::new())
		.execute(&f.store.pool)
		.await
		.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn concurrent_parent_cycle_checks_are_serialized() {
	let (f, url, schema) = setup().await;
	let workspace = f.store.create_workspace("Main", "Goal").await.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let first_task = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let second_task = f
		.store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let parent_update = Query::update()
		.table(Alias::new("tasks"))
		.value(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let mut left = f.store.pool.begin().await.unwrap();
	let mut right = f.store.pool.begin().await.unwrap();
	sqlx::query(&parent_update)
		.bind(second_task.id)
		.bind(first_task.id)
		.execute(&mut *left)
		.await
		.unwrap();
	let mut right_update = Box::pin(
		sqlx::query(&parent_update)
			.bind(first_task.id)
			.bind(second_task.id)
			.execute(&mut *right),
	);
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(100), &mut right_update)
			.await
			.is_err()
	);
	left.commit().await.unwrap();
	right_update.await.unwrap();
	let error = right.commit().await.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some("tasks_parent_cycle"), "{error}");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn cluster_and_registry_identity_constraints_match_application_bounds() {
	let (f, url, schema) = setup().await;
	let mut cluster = serde_json::to_value(model()).unwrap();
	cluster["id"] = json!("test-cluster");
	cluster["kind"] = json!("cluster");
	for config in [
		json!({}),
		json!({"coordinator":7}),
		json!({"coordinator":{"id":"bad id","version":"1.0.0"}}),
		json!({"coordinator":{"id":"agent","version":"not-semver"}}),
	] {
		let mut invalid = cluster.clone();
		invalid["config"] = config;
		check_rejected(
			insert_entry(&f.store.pool, &invalid).await,
			"registry_cluster_config",
		);
	}
	cluster["config"] = json!({"coordinator":{"id":"agent","version":"1.2.3+build.01"}});
	insert_entry(&f.store.pool, &cluster).await.unwrap();

	let mut build_only = serde_json::to_value(model()).unwrap();
	build_only["id"] = json!("build-only");
	build_only["version"] = json!("1.2.3+build.01");
	insert_entry(&f.store.pool, &build_only).await.unwrap();
	let mut oversized = serde_json::to_value(model()).unwrap();
	oversized["id"] = json!("oversized");
	oversized["version"] = json!("18446744073709551616.0.0");
	check_rejected(
		insert_entry(&f.store.pool, &oversized).await,
		"registry_semver",
	);

	let mut agent = serde_json::to_value(model()).unwrap();
	agent["id"] = json!("a".repeat(100));
	agent["version"] = json!(format!("1.0.0+{}", "a".repeat(40)));
	agent["kind"] = json!("agent");
	agent["config"] = json!({
		"model":{"id":"test-model","version":"1.0.0"},
		"instructions":"Work"
	});
	check_rejected(
		insert_entry(&f.store.pool, &agent).await,
		"registry_identity",
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn installation_constraints_validate_model_overrides() {
	let (f, url, schema) = setup().await;
	let entry = serde_json::to_value(model()).unwrap();
	insert_entry(&f.store.pool, &entry).await.unwrap();
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("test-model").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			("config", Expr::val(json!({"context_window":4096})).into()),
		],
	)
	.await
	.unwrap();
	for invalid in [
		json!({"context_window":"bad"}),
		json!({"context_window":2047}),
		json!({"provider":"unsupported"}),
		json!({"unexpected":true}),
	] {
		check_rejected(
			update(&f.store.pool, "installations", "config", Expr::val(invalid)).await,
			"installations_config",
		);
	}
	update(
		&f.store.pool,
		"installations",
		"config",
		Expr::val(json!({"context_window":8192})),
	)
	.await
	.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn agent_installation_model_overrides_keep_valid_registry_references() {
	let (f, url, schema) = setup().await;
	let mut base_model = serde_json::to_value(model()).unwrap();
	base_model["id"] = json!("base-model");
	insert_entry(&f.store.pool, &base_model).await.unwrap();
	let mut override_model = serde_json::to_value(model()).unwrap();
	override_model["id"] = json!("override-model");
	insert_entry(&f.store.pool, &override_model).await.unwrap();
	let mut wrong_kind = serde_json::to_value(model()).unwrap();
	wrong_kind["id"] = json!("not-a-model");
	wrong_kind["kind"] = json!("tool");
	wrong_kind["config"] = json!({"transport":"native","operation":"echo"});
	insert_entry(&f.store.pool, &wrong_kind).await.unwrap();
	let mut agent = serde_json::to_value(model()).unwrap();
	agent["id"] = json!("installed-agent");
	agent["kind"] = json!("agent");
	agent["config"] = json!({
		"model":{"id":"base-model","version":"1.0.0"},
		"instructions":"Work"
	});
	insert_entry(&f.store.pool, &agent).await.unwrap();
	for model_ref in [
		json!({"id":"missing-model","version":"1.0.0"}),
		json!({"id":"not-a-model","version":"1.0.0"}),
	] {
		check_rejected(
			insert_values(
				&f.store.pool,
				"installations",
				&[
					("id", Expr::val("installed-agent").into()),
					("version", Expr::val("1.0.0").into()),
					("digest", Expr::val("sha256:fixture").into()),
					("config", Expr::val(json!({"model":model_ref})).into()),
				],
			)
			.await,
			"registry_agent_model_installation_reference",
		);
	}
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("installed-agent").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			(
				"config",
				Expr::val(json!({"model":{"id":"override-model","version":"1.0.0"}})).into(),
			),
		],
	)
	.await
	.unwrap();
	let delete_override_model = Query::delete()
		.from_table(Alias::new("registry"))
		.and_where(Expr::col(Alias::new("id")).eq("override-model"))
		.and_where(Expr::col(Alias::new("version")).eq("1.0.0"))
		.to_string(PostgresQueryBuilder);
	check_rejected(
		sqlx::query(&delete_override_model)
			.execute(&f.store.pool)
			.await
			.map(|_| ()),
		"registry_agent_model_installation_reference",
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn installation_constraints_validate_effective_tool_config() {
	let (f, url, schema) = setup().await;
	let mut tool = serde_json::to_value(model()).unwrap();
	tool["id"] = json!("installed-tool");
	tool["kind"] = json!("tool");
	tool["config"] = json!({
		"transport":"http",
		"endpoint":"http://localhost:9999/base",
		"credential_env":null,
		"replay":"read_only"
	});
	insert_entry(&f.store.pool, &tool).await.unwrap();
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("installed-tool").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			("config", Expr::val(json!({})).into()),
		],
	)
	.await
	.unwrap();
	for invalid in [
		json!({"transport":"bogus"}),
		json!({"endpoint":7}),
		json!({"unexpected":true}),
	] {
		check_rejected(
			update(&f.store.pool, "installations", "config", Expr::val(invalid)).await,
			"installations_config",
		);
	}
	update(
		&f.store.pool,
		"installations",
		"config",
		Expr::val(json!({"endpoint":"http://localhost:8888/tool"})),
	)
	.await
	.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn installation_constraints_validate_skill_overrides() {
	let (f, url, schema) = setup().await;
	let mut skill = serde_json::to_value(model()).unwrap();
	skill["id"] = json!("installed-skill");
	skill["kind"] = json!("skill");
	skill["config"] = json!({"instructions":"Base instructions"});
	insert_entry(&f.store.pool, &skill).await.unwrap();
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("installed-skill").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			("config", Expr::val(json!({})).into()),
		],
	)
	.await
	.unwrap();
	for invalid in [
		json!({"instructions":7}),
		json!({"instructions":" \t\u{2003}"}),
		json!({"unexpected":true}),
	] {
		check_rejected(
			update(&f.store.pool, "installations", "config", Expr::val(invalid)).await,
			"installations_config",
		);
	}
	update(
		&f.store.pool,
		"installations",
		"config",
		Expr::val(json!({"instructions":"Override instructions"})),
	)
	.await
	.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn registry_updates_revalidate_installed_tool_overrides() {
	let (f, url, schema) = setup().await;
	let mut tool = serde_json::to_value(model()).unwrap();
	tool["id"] = json!("changing-tool");
	tool["kind"] = json!("tool");
	tool["config"] = json!({
		"transport":"http",
		"endpoint":"http://localhost:9999/base",
		"credential_env":null,
		"replay":"read_only"
	});
	insert_entry(&f.store.pool, &tool).await.unwrap();
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("changing-tool").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			(
				"config",
				Expr::val(json!({"endpoint":"http://localhost:7777/override"})).into(),
			),
		],
	)
	.await
	.unwrap();
	check_rejected(
		update_registry_config(
			&f.store.pool,
			"changing-tool",
			"1.0.0",
			json!({"transport":"native","operation":"echo"}),
		)
		.await,
		"installations_config",
	);
	update_registry_config(
		&f.store.pool,
		"changing-tool",
		"1.0.0",
		json!({
			"transport":"http",
			"endpoint":"http://localhost:8888/base",
			"credential_env":null,
			"replay":"read_only"
		}),
	)
	.await
	.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn concurrent_registry_and_installation_writes_use_one_lock_order() {
	let (f, url, schema) = setup().await;
	let mut tool = serde_json::to_value(model()).unwrap();
	tool["id"] = json!("concurrent-tool");
	tool["kind"] = json!("tool");
	tool["config"] = json!({
		"transport":"http",
		"endpoint":"http://localhost:9999/base",
		"credential_env":null,
		"replay":"read_only"
	});
	insert_entry(&f.store.pool, &tool).await.unwrap();
	insert_values(
		&f.store.pool,
		"installations",
		&[
			("id", Expr::val("concurrent-tool").into()),
			("version", Expr::val("1.0.0").into()),
			("digest", Expr::val("sha256:fixture").into()),
			("config", Expr::val(json!({})).into()),
		],
	)
	.await
	.unwrap();
	let start = std::sync::Arc::new(tokio::sync::Barrier::new(3));
	let registry_start = start.clone();
	let registry_pool = f.store.pool.clone();
	let registry_update = tokio::spawn(async move {
		registry_start.wait().await;
		update_registry_config(
			&registry_pool,
			"concurrent-tool",
			"1.0.0",
			json!({"transport":"native","operation":"echo"}),
		)
		.await
	});
	let installation_start = start.clone();
	let installation_pool = f.store.pool.clone();
	let installation_update = tokio::spawn(async move {
		installation_start.wait().await;
		update(
			&installation_pool,
			"installations",
			"config",
			Expr::val(json!({"endpoint":"http://localhost:7777/override"})),
		)
		.await
	});
	start.wait().await;
	let (registry_result, installation_result) =
		tokio::time::timeout(std::time::Duration::from_secs(5), async {
			tokio::join!(registry_update, installation_update)
		})
		.await
		.expect("registry and installation writes must not deadlock");
	let registry_result = registry_result.unwrap();
	let installation_result = installation_result.unwrap();
	assert!(registry_result.is_ok() || installation_result.is_ok());
	cleanup(f, &url, &schema).await;
}

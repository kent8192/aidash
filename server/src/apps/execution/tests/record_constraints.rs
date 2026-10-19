#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/legacy.rs"]
mod common;

static ACME: std::sync::LazyLock<aidash_application::tenancy::TenancyScope> =
	std::sync::LazyLock::new(|| aidash_application::tenancy::TenancyScope::tenant("acme").unwrap());

use aidash_server::{domain::NewTask, registry::Entry};
use common::cleanup;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::{Value, json};

fn model() -> Entry {
	serde_json::from_value(json!({
		"id":"test-model", "version":"1.0.0", "kind":"model",
		"name":{"en":"Test model"}, "description":{"en":"Fixture"},
		"config":{"provider":"openrouter", "model_id":"vendor/model",
			"endpoint":"http://localhost:9999/v1",
			"context_window":4096, "max_output_tokens":1024, "modalities":["text"], "cost":{}}
	}))
	.unwrap()
}

async fn seed_executor(f: &aidash_server::federation::Federation) {
	common::tenant_scope(&f.store, "acme").await;
	let mut definition = model();
	definition.config["context_window"] = json!(128000);
	f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			definition,
		)
		.await
		.unwrap();
	f.registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), serde_json::from_value(json!({
		"id":"executor","version":"1.0.0","kind":"agent",
		"name":{"en":"Executor"},"description":{"en":"Run codec fixture"},
		"config":{"schema_version":1,"model":{"id":"test-model","version":"1.0.0"},"instructions":"Test persisted state","bindings":[],"remove_default":[]}
	})).unwrap()).await.unwrap();
}

#[rstest::fixture]
fn executor_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> impl std::future::Future<Output = common::RuntimeFixture> {
	Box::pin(async move {
		let runtime = runtime.await;
		seed_executor(&runtime.federation).await;
		runtime
	})
}

async fn insert_entry(pool: &sqlx::PgPool, entry: &Value) -> Result<(), sqlx::Error> {
	common::tenant_root_in(pool, "acme").await;
	{
		let query_bind_1 = entry["id"].as_str().unwrap();
		let query_bind_2 = entry["version"].as_str().unwrap();
		let query_bind_3 = entry["kind"].as_str().unwrap();
		let query_bind_4 = entry;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("registry"))
				.columns(["tenant", "id", "version", "kind", "metadata"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value("acme"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(pool)
		.await
	}
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
			.value_expr(Alias::new(column), value)
			.to_string(PostgresQueryBuilder),
	)
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
		.value_expr(Alias::new("metadata"), Expr::cust("$1"))
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
	{
		let query_bind_1 = phase;
		let query_bind_2 = pending;
		let query_bind_3 = run_id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("phase"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.and_where(Expr::col(Alias::new("id")).eq(SimpleExpr::CustomWithExpr(
					"(?)".to_owned(),
					vec![Expr::value(query_bind_3.to_owned()).into()],
				)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(pool)
		.await
	}
	.map(|_| ())
}

#[rstest::rstest]
#[tokio::test]
async fn registry_constraints_reject_invalid_models_without_application_validation(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	let good = serde_json::to_value(model()).unwrap();
	for (field, invalid) in [
		("provider", json!("unsupported")),
		("model_id", json!("   ")),
		("endpoint", Value::Null),
		("context_window", json!(0)),
		("context_window", json!(2048.5)),
		("context_window", json!("4096")),
		("max_output_tokens", json!(0)),
		("max_output_tokens", json!(4097)),
		("max_output_tokens", json!(4096.5)),
		("max_output_tokens", json!("4096")),
		("modalities", json!(["audio"])),
		("reasoning_effort", json!("extreme")),
	] {
		let mut entry = good.clone();
		entry["config"][field] = invalid;
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
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
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
			"registry_model_config",
		);
	}
	let mut missing = good.clone();
	missing["config"]
		.as_object_mut()
		.unwrap()
		.remove("model_id");
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &missing).await,
		"registry_model_config",
	);
	// ADR 0006: model configurations never name an environment key.
	let mut keyed = good.clone();
	keyed["config"]["credential_env"] = json!("AIDASH_SECRET_OPENROUTER");
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &keyed).await,
		"registry_model_provider_keys",
	);
	// ADR 0007: a `chatgpt` model is served only by a Plan Connection at
	// OpenAI's endpoint, and no other model names one.
	let mut plan = good.clone();
	plan["id"] = json!("plan-model");
	plan["config"]["provider"] = json!("chatgpt");
	plan["config"]["endpoint"] = json!("https://api.openai.com/v1");
	plan["config"]["plan_connection"] = json!("chatgpt");
	for (field, invalid) in [
		("plan_connection", Value::Null),
		("plan_connection", json!("openrouter")),
		("endpoint", json!("https://openrouter.ai/api/v1")),
		("provider_credential", json!("openrouter")),
	] {
		let mut entry = plan.clone();
		entry["config"][field] = invalid;
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
			"registry_plan_connection_config",
		);
	}
	let mut borrowed = good.clone();
	borrowed["config"]["plan_connection"] = json!("chatgpt");
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &borrowed).await,
		"registry_plan_connection_config",
	);
	insert_entry(f.store.pool.fixture_driver(), &plan)
		.await
		.unwrap();
	let mut malformed = good.clone();
	malformed["name"] = json!([]);
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &malformed).await,
		"registry_metadata_shape",
	);
	for version in ["latest", "01.0.0", "1.0.0-01"] {
		let mut entry = good.clone();
		entry["version"] = json!(version);
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
			"registry_semver",
		);
	}
	insert_entry(f.store.pool.fixture_driver(), &good)
		.await
		.unwrap();
	let mut bounded = good.clone();
	bounded["id"] = json!("bounded-output-model");
	bounded["config"]["max_output_tokens"] = json!(4096);
	insert_entry(f.store.pool.fixture_driver(), &bounded)
		.await
		.unwrap();
	// An alias and a distinct version are intentionally valid; names/config are not unique keys.
	let mut alias = good.clone();
	alias["id"] = json!("alias");
	insert_entry(f.store.pool.fixture_driver(), &alias)
		.await
		.unwrap();
	alias["version"] = json!("1.1.0-rc.1+build.01");
	insert_entry(f.store.pool.fixture_driver(), &alias)
		.await
		.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"registry",
			"id",
			Expr::value("mismatch"),
		)
		.await,
		"registry_identity",
	);
	let mut whitespace = model();
	whitespace.config["model_id"] = json!("   ");
	assert!(matches!(
		aidash_server::registry::validate(&whitespace),
		Err(aidash_server::Error::Invalid(_))
	));
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn workspace_task_and_run_constraints_preserve_local_and_remote_boundaries(
	#[future(awt)]
	#[from(executor_runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let other = f
		.store
		.create_workspace(&ACME, "alice", "Other", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let task = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let foreign = f
		.store
		.create_task(&ACME, other.id, &input, "human", None)
		.await
		.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"workspaces",
			"revision",
			Expr::value(-1),
		)
		.await,
		"workspaces_content",
	);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"workspaces",
			"state",
			Expr::cust("'[]'::jsonb"),
		)
		.await,
		"workspaces_content",
	);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"tasks",
			"title",
			Expr::value(" "),
		)
		.await,
		"tasks_content",
	);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
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
			.value_expr(Alias::new("revision"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder)
	};
	check_rejected(
		sqlx::query(&set_task_revision())
			.bind(i64::MAX)
			.bind(task.id)
			.execute(f.store.pool.fixture_driver())
			.await
			.map(|_| ()),
		"tasks_content",
	);
	sqlx::query(&set_task_revision())
		.bind(i64::MAX - 1)
		.bind(task.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	check_rejected(
		sqlx::query(&set_task_revision())
			.bind(i64::MAX)
			.bind(task.id)
			.execute(f.store.pool.fixture_driver())
			.await
			.map(|_| ()),
		"tasks_content",
	);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"tasks",
			"dependencies",
			Expr::cust("ARRAY[id]"),
		)
		.await,
		"tasks_no_self_reference",
	);
	let result = {
		let query_bind_1 = foreign.id;
		let query_bind_2 = task.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("tasks"))
				.value_expr(
					Alias::new("parent_id"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.and_where(Expr::col(Alias::new("id")).eq(SimpleExpr::CustomWithExpr(
					"(?)".to_owned(),
					vec![Expr::value(query_bind_2.to_owned()).into()],
				)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
	}
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
				"tenant",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(Expr::cust("gen_random_uuid()"))
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("$2"))
				.expr(Expr::value("text"))
				.expr(Expr::value("Result"))
				.expr(Expr::cust("'{}'::jsonb"))
				.expr(Expr::value("human"))
				.expr(Expr::value("artifact-key"))
				.expr(Expr::value("acme"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&artifact)
		.bind(other.id)
		.bind(task.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap_err();
	assert_eq!(
		error.as_database_error().unwrap().constraint(),
		Some("artifacts_task_workspace")
	);
	sqlx::query(&artifact)
		.bind(workspace.id)
		.bind(task.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	// Remote runs are allowed even if their workspace/task only exist at their home node.
	let mut remote = task.clone();
	remote.id = uuid::Uuid::new_v4();
	remote.workspace_id = uuid::Uuid::new_v4();
	let run = common::remote_run(&f.store, &remote, "aidash://remote", "executor", "1.0.0").await;
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
				"tenant",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(Expr::cust("gen_random_uuid()"))
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("$2"))
				.expr(Expr::value("QUESTION"))
				.expr(Expr::value("Continue?"))
				.expr(Expr::value("question-key"))
				.expr(Expr::value("acme"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&human_request)
		.bind(workspace.id)
		.bind(run.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap_err();
	assert_eq!(
		error.as_database_error().unwrap().constraint(),
		Some("human_requests_run_workspace")
	);
	sqlx::query(&human_request)
		.bind(remote.workspace_id)
		.bind(run.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"runs",
			"step",
			Expr::value(-1),
		)
		.await,
		"runs_counters",
	);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"runs",
			"lease_owner",
			Expr::cust("gen_random_uuid()"),
		)
		.await,
		"runs_lease",
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn nonblank_constraints_match_rust_unicode_whitespace(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	insert_entry(
		f.store.pool.fixture_driver(),
		&serde_json::to_value(model()).unwrap(),
	)
	.await
	.unwrap();
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	f.store
		.create_task(
			&ACME,
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
			check_rejected(
				insert_entry(f.store.pool.fixture_driver(), &entry).await,
				constraint,
			);
		}
		for (table, column, constraint) in [
			("workspaces", "title", "workspaces_content"),
			("workspaces", "goal", "workspaces_content"),
			("tasks", "title", "tasks_content"),
			("tasks", "description", "tasks_content"),
		] {
			check_rejected(
				update(
					f.store.pool.fixture_driver(),
					table,
					column,
					Expr::value(blank.clone()),
				)
				.await,
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
				entry["config"] = if kind == "agent" {
					json!({"instructions":content, "model":{"id":"test-model", "version":"1.0.0"},"schema_version":1,"bindings":[],"remove_default":[]})
				} else {
					json!({"instructions":content})
				};
			}
			insert_entry(f.store.pool.fixture_driver(), &entry)
				.await
				.unwrap();
		}
		for (table, column) in [
			("workspaces", "title"),
			("workspaces", "goal"),
			("tasks", "title"),
			("tasks", "description"),
		] {
			update(
				f.store.pool.fixture_driver(),
				table,
				column,
				Expr::value(*content),
			)
			.await
			.unwrap();
		}
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn localized_metadata_requires_string_values_for_every_locale(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
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
				insert_entry(f.store.pool.fixture_driver(), &entry).await,
				"registry_metadata_shape",
			);
		}
	}
	let mut entry = model();
	entry.name.insert("ja".into(), "モデル".into());
	entry.description.insert("ja".into(), "説明".into());
	insert_entry(
		f.store.pool.fixture_driver(),
		&serde_json::to_value(&entry).unwrap(),
	)
	.await
	.unwrap();
	assert_eq!(
		f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&entry.id,
				&entry.version
			)
			.await
			.unwrap(),
		entry
	);
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
			.from_subquery((values.iter().map(|(_, value)| value.clone())).fold(
				Query::select(),
				|mut select, value| {
					select.expr(value);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await
	.map(|_| ())
}

#[rstest::rstest]
#[tokio::test]
async fn registry_json_shapes_remain_deserializable(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	let good = serde_json::to_value(model()).unwrap();
	for invalid_schema in [
		json!({"type":7}),
		json!({"items":7}),
		json!({"properties":{"count":{"required":"name"}}}),
	] {
		let mut invalid = good.clone();
		invalid["schema"] = invalid_schema;
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &invalid).await,
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
				insert_entry(f.store.pool.fixture_driver(), &entry).await,
				"registry_metadata_shape",
			);
		}
	}
	let mut unknown = good.clone();
	unknown["unexpected"] = json!(true);
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &unknown).await,
		"registry_metadata_shape",
	);
	for field in ["capabilities", "tags", "languages", "skills"] {
		unknown.as_object_mut().unwrap().remove(field);
	}
	unknown.as_object_mut().unwrap().remove("unexpected");
	insert_entry(f.store.pool.fixture_driver(), &unknown)
		.await
		.unwrap();
	assert_eq!(
		f.registry
			.list(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&Default::default()
			)
			.await
			.unwrap()
			.len(),
		1
	);
	let mut full = good;
	full["id"] = json!("full");
	for field in ["capabilities", "tags", "languages", "skills"] {
		full[field] = json!(["one", "two"]);
	}
	insert_entry(f.store.pool.fixture_driver(), &full)
		.await
		.unwrap();
	assert_eq!(
		f.registry
			.list(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				&Default::default()
			)
			.await
			.unwrap()
			.len(),
		2
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn model_and_agent_configs_reject_unusable_shapes(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
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
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
			"registry_model_config",
		);
	}
	let mut missing = good.clone();
	missing["config"].as_object_mut().unwrap().remove("cost");
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &missing).await,
		"registry_model_config",
	);
	let mut agent = good.clone();
	agent["id"] = json!("agent");
	agent["kind"] = json!("agent");
	agent["config"] =
		json!({"schema_version":1,"bindings":[],"remove_default":[],"instructions":"Work"});
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &agent).await,
		"registry_agent_config",
	);
	insert_entry(f.store.pool.fixture_driver(), &good)
		.await
		.unwrap();
	let mut other_kind = serde_json::to_value(model()).unwrap();
	other_kind["id"] = json!("not-a-model");
	other_kind["kind"] = json!("tool");
	other_kind["config"] = json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://localhost:9999/tool","replay":"read_only"}});
	insert_entry(f.store.pool.fixture_driver(), &other_kind)
		.await
		.unwrap();
	let mut skill = serde_json::to_value(model()).unwrap();
	skill["id"] = json!("test-skill");
	skill["kind"] = json!("skill");
	skill["config"] = json!({"instructions":"Help with a focused task"});
	insert_entry(f.store.pool.fixture_driver(), &skill)
		.await
		.unwrap();
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
			insert_entry(f.store.pool.fixture_driver(), &agent).await,
			"registry_agent_config",
		);
	}
	agent["config"]["model"] = json!({"id":"test-model","version":"1.0.0"});
	let mut missing_model = agent.clone();
	missing_model["id"] = json!("missing-model-agent");
	missing_model["config"]["model"] = json!({"id":"does-not-exist","version":"1.0.0"});
	check_foreign_key_rejected(
		insert_entry(f.store.pool.fixture_driver(), &missing_model).await,
		"registry_agent_model_target",
	);
	let mut wrong_kind_model = agent.clone();
	wrong_kind_model["id"] = json!("wrong-kind-model-agent");
	wrong_kind_model["config"]["model"] = json!({"id":"not-a-model","version":"1.0.0"});
	check_foreign_key_rejected(
		insert_entry(f.store.pool.fixture_driver(), &wrong_kind_model).await,
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
			insert_entry(f.store.pool.fixture_driver(), &malformed).await,
			"registry_agent_config",
		);
	}
	let mut fractional_steps = agent.clone();
	fractional_steps["config"]["max_steps"] = json!(1.0);
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &fractional_steps).await,
		"registry_agent_config",
	);
	let mut integer_steps = agent.clone();
	integer_steps["id"] = json!("integer-agent");
	integer_steps["config"]["max_steps"] = json!(1);
	let _: aidash_server::registry::AgentConfig =
		serde_json::from_value(integer_steps["config"].clone()).unwrap();
	insert_entry(f.store.pool.fixture_driver(), &integer_steps)
		.await
		.unwrap();
	let _: aidash_server::registry::AgentConfig =
		serde_json::from_value(agent["config"].clone()).unwrap();
	insert_entry(f.store.pool.fixture_driver(), &agent)
		.await
		.unwrap();
	let mut cluster = serde_json::to_value(model()).unwrap();
	cluster["id"] = json!("test-cluster");
	cluster["kind"] = json!("cluster");
	cluster["config"] = json!({"coordinator":{"id":"agent","version":"1.0.0"}});
	insert_entry(f.store.pool.fixture_driver(), &cluster)
		.await
		.unwrap();
	let mut linked_agent_config = agent["config"].clone();
	linked_agent_config["bindings"] = json!([{"kind":"tool","target":{"registry_node":"aidash://execution-test","id":"not-a-model","version":"1.0.0"},"narrow":{}},{"kind":"skill","target":{"registry_node":"aidash://execution-test","id":"test-skill","version":"1.0.0"},"narrow":{}}]);
	linked_agent_config["cluster"] = json!({"id":"test-cluster","version":"1.0.0"});
	update_registry_config(
		f.store.pool.fixture_driver(),
		"agent",
		"1.0.0",
		linked_agent_config,
	)
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
		if field == "cluster" {
			check_foreign_key_rejected(
				insert_entry(f.store.pool.fixture_driver(), &invalid_reference).await,
				"registry_agent_resource_target",
			);
		} else {
			check_rejected(
				insert_entry(f.store.pool.fixture_driver(), &invalid_reference).await,
				"registry_agent_config",
			);
		}
	}
	// TRUNCATE skips row-level security, so the server role may not issue it.
	let refused = sqlx::query("TRUNCATE registry_agent_model_refs")
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap_err();
	assert_eq!(
		refused
			.as_database_error()
			.and_then(|error| error.code())
			.as_deref(),
		Some("42501"),
		"{refused}"
	);
	// Even the table owner cannot TRUNCATE past the reference triggers.
	let owner = common::owner_pool(f.store.pool.fixture_driver()).await;
	let truncate_model_refs = sqlx::query("TRUNCATE registry_agent_model_refs")
		.execute(&owner)
		.await
		.map(|_| ());
	check_rejected(truncate_model_refs, "registry_agent_model_reference");
	let truncate_resource_refs = sqlx::query("TRUNCATE registry_agent_resource_refs")
		.execute(&owner)
		.await
		.map(|_| ());
	check_rejected(truncate_resource_refs, "registry_agent_resource_reference");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn tool_configs_reject_undecodable_shapes(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	let mut tool = serde_json::to_value(model()).unwrap();
	tool["id"] = json!("tool");
	tool["kind"] = json!("tool");
	for config in [
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"not a URL","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"ftp://example.com","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http:///missing-host","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"https://user:pass@example.com","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://example.com?token=value","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"https://example.com/path#fragment","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.mcp@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"mcp","endpoint":"http://example.com?token=value","tool_name":"call","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"native","operation":"http_get","allowed_hosts":[]}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://localhost","replay":"invalid"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.mcp@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"mcp","endpoint":"http://localhost","tool_name":"call","replay":"idempotent"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.mcp@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"mcp","endpoint":"http://localhost","tool_name":"call","replay":"idempotent","idempotency_argument":" \t\n\u{2003}"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.agent@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"agent","node_id":"bad node","agent":{"id":"agent","version":"1.0.0"}}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://localhost","replay":"read_only","unexpected":true}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":"http://localhost","credential_env":"OPENAI_API_KEY","replay":"read_only"}}),
		json!({"registry_node":"aidash://execution-test","provider":"integration.mcp@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{"transport":"mcp","endpoint":"http://localhost","credential_env":"SECRET_TOKEN","tool_name":"call","replay":"read_only"}}),
	] {
		tool["config"] = config;
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &tool).await,
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
		tool["config"] = json!({"registry_node":"aidash://execution-test","provider":"integration.http@1","operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":{
			"transport":"http",
			"endpoint":endpoint,
			"replay":"read_only"
		}});
		insert_entry(f.store.pool.fixture_driver(), &tool)
			.await
			.unwrap();
	}
	// A Tool never names a Deployment secret, even a well-formed one (#296).
	tool["id"] = json!("secret-tool");
	for (provider, transport) in [
		(
			"integration.http@1",
			json!({"transport":"http","endpoint":"http://localhost","credential_env":"AIDASH_SECRET_TOOL_TOKEN","replay":"read_only"}),
		),
		(
			"integration.http@1",
			json!({"transport":"http","endpoint":"http://localhost","credential_env":null,"replay":"read_only"}),
		),
		(
			"integration.mcp@1",
			json!({"transport":"mcp","endpoint":"http://localhost","credential_env":"AIDASH_SECRET_TOOL_TOKEN","tool_name":"call","replay":"read_only"}),
		),
	] {
		tool["config"] = json!({"registry_node":"aidash://execution-test","provider":provider,"operation":"invoke","default_alias":"fixture","tier":"integration","narrow":{},"transport":transport});
		check_rejected(
			insert_entry(f.store.pool.fixture_driver(), &tool).await,
			"registry_tool_config",
		);
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_decider_kind_replaces_compactors_and_embedding_configs_reject_undecodable_shapes(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	let mut entry = serde_json::to_value(model()).unwrap();
	// The compactor kind is gone (#108); compaction runs through a Decider,
	// whose configuration the application validates.
	entry["id"] = json!("compactor");
	entry["kind"] = json!("compactor");
	entry["config"] = json!({"provider_credential":"openrouter","model":"typesafe/jev-1.13"});
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &entry).await,
		"registry_kind_check",
	);
	entry["id"] = json!("decider");
	entry["kind"] = json!("decider");
	entry["config"] = json!({
		"hook":"compaction","answer_type":"noul","description":"Keep needed history",
		"provider_credential":"openrouter",
		"routes":[{"provider_contract":"typesafe.jev/1","model":"typesafe/jev-1.14","keep_threshold":"3fe0000000000000"}],
		"builder":"aidash.compaction/2",
		"option_source":"history_event/1","rule":"compaction.keep/1",
		"mode":"enforce","max_calls_per_run":20
	});
	insert_entry(f.store.pool.fixture_driver(), &entry)
		.await
		.unwrap();

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
			insert_entry(f.store.pool.fixture_driver(), &entry).await,
			"registry_embedding_config",
		);
	}
	entry["config"] = json!({
		"provider":"openai",
		"endpoint":"http://localhost:9999/v1",
		"model":"embedding-model",
		"model_version":"1",
		"dimensions":1536
	});
	insert_entry(f.store.pool.fixture_driver(), &entry)
		.await
		.unwrap();
	entry["id"] = json!("openrouter-embedding");
	entry["config"]["provider"] = json!("openrouter");
	entry["config"]["endpoint"] = json!("https://openrouter.ai/api/v1");
	entry["config"]["model"] = json!("google/gemini-embedding-2");
	entry["config"]["dimensions"] = json!(3072);
	insert_entry(f.store.pool.fixture_driver(), &entry)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn requirements_constraints_and_strict_run_codec_reject_wrong_shapes(
	#[future(awt)]
	#[from(executor_runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			&ACME,
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
		.value_expr(Alias::new("status"), Expr::value("CLAIMED"))
		.value_expr(Alias::new("owner"), Expr::cust("NULL"))
		.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(task.id)))
		.to_string(PostgresQueryBuilder);
	check_rejected(
		sqlx::query(&claimed_without_owner)
			.execute(f.store.pool.fixture_driver())
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
			update(
				f.store.pool.fixture_driver(),
				"tasks",
				"requirements",
				Expr::value(invalid),
			)
			.await,
			"tasks_content",
		);
	}
	let valid = json!({"kind":null,"query":"text","capability":"code","language":"en","skill":"read","tag":"tag","model":"model"});
	let _: aidash_server::registry::Search = serde_json::from_value(valid.clone()).unwrap();
	update(
		f.store.pool.fixture_driver(),
		"tasks",
		"requirements",
		Expr::value(valid),
	)
	.await
	.unwrap();
	let run = common::remote_run(&f.store, &task, "aidash://remote", "executor", "1.0.0").await;
	// Storage accepts damaged machine payloads so metadata controls and the
	// fenced failure path remain available. The codec is the shape authority.
	for column in ["context", "pending"] {
		for invalid in [Value::Null, json!([]), json!(7), json!("text"), json!({})] {
			update(
				f.store.pool.fixture_driver(),
				"runs",
				column,
				Expr::value(invalid),
			)
			.await
			.unwrap();
			assert!(f.store.run(&ACME, run.id).await.is_err());
			assert!(
				f.store
					.inspect_run(&ACME, run.id)
					.await
					.unwrap()
					.state_error
					.is_some()
			);
		}
		update(
			f.store.pool.fixture_driver(),
			"runs",
			column,
			Expr::value(if column == "context" {
				common::context(json!({}))
			} else {
				common::pending(aidash_server::domain::RunState::default())
			}),
		)
		.await
		.unwrap();
	}
	let human = |id| {
		common::pending(aidash_server::domain::RunState::Waiting(Box::new(
			aidash_server::domain::WaitingState::Human {
				request_id: id,
				resume: aidash_server::domain::ResumeState::Thinking(Default::default()),
			},
		)))
	};
	check_foreign_key_rejected(
		update_run_state(
			f.store.pool.fixture_driver(),
			run.id,
			"WAITING",
			human(uuid::Uuid::new_v4()),
		)
		.await,
		"runs_human_request_ref",
	);
	let request = f
		.store
		.human_request(&ACME, &run, "QUESTION", "Continue?", "waiting-shape-test")
		.await
		.unwrap();
	update_run_state(
		f.store.pool.fixture_driver(),
		run.id,
		"WAITING",
		human(request.id),
	)
	.await
	.unwrap();
	check_foreign_key_rejected(
		update(
			f.store.pool.fixture_driver(),
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
			&ACME,
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
	let other_run = common::remote_run(
		&f.store,
		&other_task,
		"aidash://remote",
		"executor",
		"1.0.0",
	)
	.await;
	check_foreign_key_rejected(
		update(
			f.store.pool.fixture_driver(),
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
			.execute(f.store.pool.fixture_driver())
			.await
			.map(|_| ()),
		"runs_human_request_ref",
	);
	update_run_state(
		f.store.pool.fixture_driver(),
		other_run.id,
		"COMPLETED",
		common::pending(aidash_server::domain::RunState::Completed(
			aidash_server::domain::TerminalState {},
		)),
	)
	.await
	.unwrap();
	update_run_state(
		f.store.pool.fixture_driver(),
		run.id,
		"READY",
		common::pending(aidash_server::domain::RunState::default()),
	)
	.await
	.unwrap();
	let valid = common::tool_pending(
		json!({"response":{"text":"","tool_calls":[{"id":"call-1","name":"echo","arguments":{}}]},"cursor":1}),
	);
	update_run_state(
		f.store.pool.fixture_driver(),
		run.id,
		"TOOL_CALL",
		valid.clone(),
	)
	.await
	.unwrap();
	assert!(f.store.run(&ACME, run.id).await.is_ok());
	for (field, value) in [
		("cursor", json!(2)),
		("response_epoch", json!(-1)),
		("future_key", json!(true)),
	] {
		let mut invalid = valid.clone();
		invalid["data"][field] = value;
		update_run_state(f.store.pool.fixture_driver(), run.id, "TOOL_CALL", invalid)
			.await
			.unwrap();
		assert!(f.store.run(&ACME, run.id).await.is_err());
	}
	update_run_state(
		f.store.pool.fixture_driver(),
		run.id,
		"READY",
		common::pending(aidash_server::domain::RunState::default()),
	)
	.await
	.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"runs",
			"revision",
			Expr::value(i64::MAX),
		)
		.await,
		"runs_counters",
	);
	let second_task = f
		.store
		.create_task(
			&ACME,
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
	let second_run = common::remote_run(
		&f.store,
		&second_task,
		"aidash://remote",
		"executor",
		"1.0.0",
	)
	.await;
	let max_safe_revision = Query::update()
		.table(Alias::new("runs"))
		.value_expr(Alias::new("revision"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&max_safe_revision)
		.bind(i64::MAX - 1)
		.bind(run.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query(&max_safe_revision)
		.bind(i64::MAX - 2)
		.bind(second_run.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let third_task = f
		.store
		.create_task(
			&ACME,
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
	let third_run = common::remote_run(
		&f.store,
		&third_task,
		"aidash://remote",
		"executor",
		"1.0.0",
	)
	.await;
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
		json!({"compactions":-1}),
		json!({"compactions":4294967296_u64}),
		common::context(json!({"history":[{"kind":"future_event"}]})),
	] {
		update(
			f.store.pool.fixture_driver(),
			"runs",
			"context",
			Expr::value(malformed),
		)
		.await
		.unwrap();
		assert!(
			f.store
				.inspect_run(&ACME, run.id)
				.await
				.unwrap()
				.state_error
				.is_some()
		);
	}
	update(
		f.store.pool.fixture_driver(),
		"runs",
		"context",
		Expr::value(common::context(json!({}))),
	)
	.await
	.unwrap();
	let lease_deadline_update = |deadline: &str| {
		Query::update()
			.table(Alias::new("runs"))
			.value_expr(Alias::new("lease_owner"), Expr::cust("gen_random_uuid()"))
			.value_expr(
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
				.execute(f.store.pool.fixture_driver())
				.await
				.map(|_| ()),
			"runs_lease",
		);
	}
	sqlx::query(&lease_deadline_update("2030-01-01 00:00:00+00"))
		.bind(run.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

fn index_spec() -> Value {
	json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9999/v1","model":"embedding","model_version":"1","dimensions":3},
        "vector":{"provider":"postgres","endpoint":"local"},
        "enabled":true,"auto_context":false,"max_sources":64,"max_results":10,"max_result_tokens":4096,"max_input_bytes":8192})
}

#[rstest::rstest]
#[tokio::test]
async fn semantic_specs_and_sources_reject_undecodable_records(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let spec = index_spec();
	let _: aidash_server::semantic::IndexSpec = serde_json::from_value(spec.clone()).unwrap();
	insert_values(
		f.store.pool.fixture_driver(),
		"semantic_indexes",
		&[
			("workspace_id", uuid_expr(workspace.id)),
			("tenant", Expr::value("acme").into()),
			("revision", Expr::value(1).into()),
			("spec", Expr::value(spec.clone()).into()),
			("collection", Expr::value("test").into()),
		],
	)
	.await
	.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"semantic_indexes",
			"revision",
			Expr::value(i64::MAX),
		)
		.await,
		"semantic_indexes_revision",
	);
	let mut empty_port = spec.clone();
	empty_port["embedding"]["endpoint"] = json!("http://localhost:");
	let runtime: aidash_server::semantic::IndexSpec =
		serde_json::from_value(empty_port.clone()).unwrap();
	runtime.validate().unwrap();
	update(
		f.store.pool.fixture_driver(),
		"semantic_indexes",
		"spec",
		Expr::value(empty_port),
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
			let runtime: aidash_server::semantic::IndexSpec =
				serde_json::from_value(boundary.clone()).unwrap();
			runtime.validate().unwrap();
			update(
				f.store.pool.fixture_driver(),
				"semantic_indexes",
				"spec",
				Expr::value(boundary),
			)
			.await
			.unwrap();
		}
	}
	for value in [1, 8192] {
		let mut boundary = spec.clone();
		boundary["embedding"]["dimensions"] = json!(value);
		let runtime: aidash_server::semantic::IndexSpec =
			serde_json::from_value(boundary.clone()).unwrap();
		runtime.validate().unwrap();
		update(
			f.store.pool.fixture_driver(),
			"semantic_indexes",
			"spec",
			Expr::value(boundary),
		)
		.await
		.unwrap();
	}
	update(
		f.store.pool.fixture_driver(),
		"semantic_indexes",
		"spec",
		Expr::value(spec.clone()),
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
		// An embedding credential_env is rejected by its own constraint below.
		let mut fields = vec![("unexpected", json!(true))];
		if section == "vector" {
			fields.push(("credential_env", json!(7)));
		}
		for (field, value) in fields {
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
				f.store.pool.fixture_driver(),
				"semantic_indexes",
				"spec",
				Expr::value(invalid),
			)
			.await,
			"semantic_indexes_revision",
		);
	}
	// ADR 0006: an embedding never names an environment key, even a valid one.
	let mut keyed = spec.clone();
	keyed["embedding"]["credential_env"] = json!("AIDASH_SECRET_EMBEDDING");
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"semantic_indexes",
			"spec",
			Expr::value(keyed),
		)
		.await,
		"semantic_embedding_provider_keys",
	);
	insert_values(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		&[
			("id", uuid_expr(uuid::Uuid::new_v4())),
			("tenant", Expr::value("acme").into()),
			("workspace_id", uuid_expr(workspace.id)),
			("key", Expr::value("entry").into()),
			(
				"source",
				Expr::value(json!({"kind":"memory","text":"text"})).into(),
			),
			("metadata", Expr::value(json!({})).into()),
			("revision", Expr::value(1).into()),
			("point_id", uuid_expr(uuid::Uuid::new_v4())),
			("index_revision", Expr::value(1).into()),
			("state", Expr::value("PENDING").into()),
			("created_by", Expr::value("human").into()),
			(
				"authority",
				Expr::value(json!({
					"credential": null,
					"tenant": "",
					"subject": "without_subject",
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
		json!({"credential":null,"tenant":"","subject":"without_subject","subjects":[7]}),
		json!({"credential":"not-a-uuid","tenant":"","subject":"without_subject","subjects":[]}),
		json!({"credential":null,"tenant":7,"subject":"without_subject","subjects":[]}),
	] {
		check_rejected(
			update(
				f.store.pool.fixture_driver(),
				"semantic_entries",
				"authority",
				Expr::value(invalid),
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
				f.store.pool.fixture_driver(),
				"semantic_entries",
				"source",
				Expr::value(invalid),
			)
			.await,
			"semantic_entries_counters",
		);
	}
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"deleted",
		Expr::value(true),
	)
	.await
	.unwrap();
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"source",
		Expr::value(json!({"kind":"memory","text":""})),
	)
	.await
	.unwrap();
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"source",
		Expr::value(json!({"kind":"memory","text":"text"})),
	)
	.await
	.unwrap();
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"deleted",
		Expr::value(false),
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
			let _: aidash_server::semantic::Source =
				serde_json::from_value(source.clone()).unwrap();
			update(
				f.store.pool.fixture_driver(),
				"semantic_entries",
				"source",
				Expr::value(source),
			)
			.await
			.unwrap();
		}
	}
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"source",
		Expr::value(json!({"kind":"memory","text":"x".repeat(129)})),
	)
	.await
	.unwrap();
	let mut smaller_limit = spec.clone();
	smaller_limit["max_input_bytes"] = json!(128);
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"semantic_indexes",
			"spec",
			Expr::value(smaller_limit.clone()),
		)
		.await,
		"semantic_index_input_bytes",
	);
	update(
		f.store.pool.fixture_driver(),
		"semantic_entries",
		"source",
		Expr::value(json!({"kind":"memory","text":"short"})),
	)
	.await
	.unwrap();
	update(
		f.store.pool.fixture_driver(),
		"semantic_indexes",
		"spec",
		Expr::value(smaller_limit),
	)
	.await
	.unwrap();
	check_rejected(
		update(
			f.store.pool.fixture_driver(),
			"semantic_entries",
			"source",
			Expr::value(json!({"kind":"memory","text":"é".repeat(65)})),
		)
		.await,
		"semantic_memory_input_bytes",
	);
	cleanup(f, &url, &schema).await;
}

fn dependencies_update() -> String {
	Query::update()
		.table(Alias::new("tasks"))
		.value_expr(Alias::new("dependencies"), Expr::cust("$2"))
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

#[rstest::rstest]
#[tokio::test]
async fn concurrent_dependency_changes_cannot_race_target_deletion(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	use std::time::Duration;
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let task = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	for deletion_first in [true, false] {
		let target = f
			.store
			.create_task(&ACME, workspace.id, &input, "human", None)
			.await
			.unwrap();
		let mut transaction = f.store.pool.fixture_driver().begin().await.unwrap();
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
		let pool = f.store.pool.fixture_driver().clone();
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
	Expr::cust(format!("'{id}'::uuid")).into()
}

#[rstest::rstest]
#[tokio::test]
async fn task_parent_cycle_guard_rejects_direct_cycles(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let parent = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let child = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let parent_update = Query::update()
		.table(Alias::new("tasks"))
		.value_expr(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&parent_update)
		.bind(child.id)
		.bind(parent.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let error = sqlx::query(&parent_update)
		.bind(parent.id)
		.bind(child.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap_err();
	let database = error.as_database_error().unwrap();
	assert_eq!(database.code().as_deref(), Some("23514"), "{error}");
	assert_eq!(database.constraint(), Some("tasks_parent_cycle"), "{error}");
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn task_dependency_cycles_include_parent_edges(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let first = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let second = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let set_dependencies = Query::update()
		.table(Alias::new("tasks"))
		.value_expr(Alias::new("dependencies"), Expr::cust("ARRAY[$1::uuid]"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&set_dependencies)
		.bind(first.id)
		.bind(second.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let error = sqlx::query(&set_dependencies)
		.bind(second.id)
		.bind(first.id)
		.execute(f.store.pool.fixture_driver())
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
		.value_expr(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let error = sqlx::query(&set_parent)
		.bind(first.id)
		.bind(second.id)
		.execute(f.store.pool.fixture_driver())
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

#[rstest::rstest]
#[tokio::test]
async fn task_cycle_checks_deduplicate_diamond_reachability(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
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
					.create_task(&ACME, workspace.id, &input, "human", None)
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
					.execute(f.store.pool.fixture_driver())
					.await
					.unwrap();
			}
		}
		levels.push(level);
	}
	let error = sqlx::query(&dependencies_update())
		.bind(levels[0][0])
		.bind(vec![levels.last().unwrap()[0]])
		.execute(f.store.pool.fixture_driver())
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

#[rstest::rstest]
#[tokio::test]
async fn concurrent_parent_cycle_checks_are_serialized(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let first_task = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let second_task = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let parent_update = Query::update()
		.table(Alias::new("tasks"))
		.value_expr(Alias::new("parent_id"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let mut left = f.store.pool.fixture_driver().begin().await.unwrap();
	let mut right = f.store.pool.fixture_driver().begin().await.unwrap();
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

#[rstest::rstest]
#[tokio::test]
async fn cluster_and_registry_identity_constraints_match_application_bounds(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
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
			insert_entry(f.store.pool.fixture_driver(), &invalid).await,
			"registry_cluster_config",
		);
	}
	cluster["config"] = json!({"coordinator":{"id":"agent","version":"1.2.3+build.01"}});
	insert_entry(f.store.pool.fixture_driver(), &cluster)
		.await
		.unwrap();

	let mut build_only = serde_json::to_value(model()).unwrap();
	build_only["id"] = json!("build-only");
	build_only["version"] = json!("1.2.3+build.01");
	insert_entry(f.store.pool.fixture_driver(), &build_only)
		.await
		.unwrap();
	let mut oversized = serde_json::to_value(model()).unwrap();
	oversized["id"] = json!("oversized");
	oversized["version"] = json!("18446744073709551616.0.0");
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &oversized).await,
		"registry_semver",
	);

	let mut agent = serde_json::to_value(model()).unwrap();
	agent["id"] = json!("a".repeat(100));
	agent["version"] = json!(format!("1.0.0+{}", "a".repeat(40)));
	agent["kind"] = json!("agent");
	agent["config"] = json!({
		"model":{"id":"test-model","version":"1.0.0"},
		"instructions":"Work", "schema_version":1,"bindings":[],"remove_default":[]
	});
	check_rejected(
		insert_entry(f.store.pool.fixture_driver(), &agent).await,
		"registry_identity",
	);
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

#[rstest::rstest]
#[tokio::test]
async fn task_dependencies_enforce_existence_ownership_and_reverse_changes(
	#[future(awt)]
	#[from(common::runtime)]
	fixture: common::RuntimeFixture,
) {
	let (f, url, schema) = fixture.parts();
	common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Main", "Goal")
		.await
		.unwrap();
	let other = f
		.store
		.create_workspace(&ACME, "alice", "Other", "Goal")
		.await
		.unwrap();
	let input = NewTask {
		title: "Task".into(),
		description: "Work".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let task = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let target = f
		.store
		.create_task(&ACME, workspace.id, &input, "human", None)
		.await
		.unwrap();
	let foreign = f
		.store
		.create_task(&ACME, other.id, &input, "human", None)
		.await
		.unwrap();
	let query = dependencies_update();
	for dependency in [uuid::Uuid::new_v4(), foreign.id] {
		dependency_error(
			sqlx::query(&query)
				.bind(task.id)
				.bind(vec![dependency])
				.execute(f.store.pool.fixture_driver())
				.await
				.unwrap_err(),
		);
	}
	// Bypassing the application on insertion must reject a foreign dependency too.
	dependency_error(
		insert_values(
			f.store.pool.fixture_driver(),
			"tasks",
			&[
				("id", uuid_expr(uuid::Uuid::new_v4())),
				("tenant", Expr::value("acme").into()),
				("workspace_id", uuid_expr(workspace.id)),
				("title", Expr::value("Direct").into()),
				("created_by", Expr::value("human").into()),
				("description", Expr::value("Work").into()),
				(
					"dependencies",
					Expr::cust(format!("ARRAY['{}'::uuid]", foreign.id)).into(),
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
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	dependency_error(
		sqlx::query(&delete_task())
			.bind(target.id)
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap_err(),
	);
	let move_target = Query::update()
		.table(Alias::new("tasks"))
		.value_expr(Alias::new("workspace_id"), uuid_expr(other.id))
		.and_where(Expr::col(Alias::new("id")).eq(uuid_expr(target.id)))
		.to_string(PostgresQueryBuilder);
	dependency_error(
		sqlx::query(&move_target)
			.execute(f.store.pool.fixture_driver())
			.await
			.unwrap_err(),
	);
	check_rejected(
		sqlx::query(
			&Query::delete()
				.from_table(Alias::new("task_dependencies"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.fixture_driver())
		.await
		.map(|_| ()),
		"tasks_dependencies_managed",
	);

	// The fresh baseline retains the same ownership and reverse-edge guards.
	sqlx::query(&dependencies_update())
		.bind(task.id)
		.bind(Vec::<uuid::Uuid>::new())
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query(&delete_task())
		.bind(target.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

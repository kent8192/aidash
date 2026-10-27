//! Fresh-schema verification for the native Reinhardt migration command.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/deployment.rs"]
mod deployment;
use aidash_server::apps::workspaces::models::Workspace;
use deployment::{deployment_command, deployment_command_excluding};
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::migrations::{
	DatabaseMigrationRecorder, FilesystemSource, MigrationSource, SqlDialect,
	build_state_from_files,
};
use reinhardt::db::orm::Model;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use reinhardt::test::fixtures::temp_dir;
#[path = "support/postgres.rs"]
mod postgres;
use postgres::postgres_container;
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use rstest::{fixture, rstest};
use serde_json::json;
use sqlx::PgPool;
use std::{
	collections::BTreeSet, fs::File, path::PathBuf, process::Stdio, sync::Arc, time::Duration,
};
use tempfile::TempDir;
use uuid::Uuid;

/// Issue #243's migrations and the registry migrations that depend on them.
/// Historical rollback tests run on the history before them, because reversing
/// an earlier migration from the current head would reach objects those
/// migrations removed or replaced. A later registry migration left out of this
/// list loses its dependency in that history and can run before registry/0001.
const TENANT_REGISTRY_MIGRATIONS: &[(&str, &str)] = &[
	("registry", "0026_remove_legacy_installations"),
	("registry", "0027_remove_legacy_installations_state"),
	("registry", "0028_tool_transport_secrets"),
	("registry", "0029_completion_policy"),
	("registry", "0030_plan_connection_models"),
	("registry", "0031_model_request_estimator"),
	("registry", "0032_degraded_context"),
	("registry", "0033_row_level_security"),
	("registry", "0034_research_view"),
	("registry", "0035_decider_kind"),
	("registry", "0036_decider_kind_state"),
	("registry", "0037_tenant_owned_row_level_security"),
	("registry", "0038_agent_plugin_copies"),
	("registry", "0039_agent_plugin_copies_state"),
	("registry", "0040_agent_plugin_copies_row_level_security"),
	("registry", "0041_agent_plugins_definition"),
	("registry", "0042_external_services"),
	("registry", "0043_external_services_state"),
	("registry", "0044_foreign_key_relations_state"),
	("registry", "0045_context_sources"),
	("registry", "0045_agent_plugin_settings"),
	("registry", "0045_projection_measurement_config"),
	("registry", "0046_context_projection_merge"),
	("registry", "0046_merge_agent_plugin_and_projection"),
	("registry", "0046_marketplace_recalls"),
	("registry", "0047_marketplace_recall_constraints"),
	("registry", "0048_context_marketplace_merge"),
	("registry", "0048_plugin_marketplace_merge"),
	("registry", "0049_plugin_context_merge"),
	("marketplace", "0008_remove_compatibility_gate"),
	("marketplace", "0009_remove_compatibility_gate_state"),
	("marketplace", "0010_row_level_security"),
	("marketplace", "0011_tenant_owned_row_level_security"),
];

async fn history_size() -> usize {
	FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
		.all_migrations()
		.await
		.unwrap()
		.len()
}

#[rstest]
#[tokio::test]
async fn native_history_uses_typed_schema_operations_and_lf_sql_assets() {
	// Arrange: the native history is the only schema identity source.
	let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
	// Act: load every external SQL asset through the native filesystem source.
	let migrations = FilesystemSource::new(&root).all_migrations().await.unwrap();
	// A new migration must extend its app's single head, including state-only
	// checkpoints; migrate alone can otherwise accept a forked graph.
	let mut graph = reinhardt::db::migrations::MigrationGraph::new();
	for migration in &migrations {
		graph.add_migration(
			reinhardt::db::migrations::MigrationKey::new(&migration.app_label, &migration.name),
			migration
				.dependencies
				.iter()
				.map(|(app, name)| reinhardt::db::migrations::MigrationKey::new(app, name))
				.collect(),
		);
	}
	assert!(
		graph.detect_conflicts().is_empty(),
		"every app must have at most one global leaf"
	);
	let knowledge = graph.get_leaf_nodes_for_app("knowledge");
	assert_eq!(knowledge.len(), 1);
	assert_eq!(knowledge[0].name, "0037_recovery_evidence_merge");
	// Assert: retain the physical graph, model snapshots, and all supported tables.
	assert!(
		migrations
			.iter()
			.any(|m| m.name == "0022_memory_receiver_caches")
	);
	assert_eq!(
		migrations
			.iter()
			.filter(|migration| migration.state_only)
			.count(),
		66
	);
	let tables = migrations
		.iter()
		.filter(|migration| !migration.state_only)
		.flat_map(|migration| &migration.operations)
		.filter(|operation| {
			matches!(
				operation,
				reinhardt::db::migrations::Operation::CreateTable { .. }
			)
		})
		.count();
	assert!(tables >= 117, "retain every baseline and new memory table");
	// The repository allocates each Run's progress sequence; the physical
	// column has no identity, so the logical state must not claim one.
	let progress_seq = migrations
		.iter()
		.filter(|migration| migration.state_only)
		.flat_map(|migration| &migration.operations)
		.find_map(|operation| match operation {
			reinhardt::db::migrations::Operation::CreateTable { name, columns, .. }
				if name == "inference_progress" =>
			{
				columns.iter().find(|column| column.name == "seq")
			}
			_ => None,
		})
		.unwrap();
	assert!(progress_seq.primary_key && !progress_seq.auto_increment);
	for migration in migrations
		.iter()
		.filter(|migration| migration.database_only)
	{
		for operation in &migration.operations {
			if let reinhardt::db::migrations::Operation::RunSQL { sql, reverse_sql } = operation {
				assert!(
					reverse_sql.is_some(),
					"{}.{}",
					migration.app_label,
					migration.name
				);
				assert!(
					!sql.starts_with("CREATE TABLE")
						&& !sql.starts_with("CREATE INDEX")
						&& !sql.starts_with("CREATE UNIQUE INDEX"),
					"{}: {sql}",
					migration.name
				);
				if sql.starts_with("ALTER TABLE")
					&& (migration.app_label.as_str(), migration.name.as_str())
						== ("identity", "0020_tenancy_schema")
				{
					// The pinned typed AlterColumn renders no NOT NULL change.
					assert_eq!(
						sql.trim(),
						"ALTER TABLE public.dashboard_administration_history ALTER COLUMN identity_id DROP NOT NULL;"
					);
				} else if sql.starts_with("ALTER TABLE")
					&& migration.name.ends_with("_row_level_security")
				{
					// Reinhardt has no row-level security operations (#6770). The
					// database::policy unit tests compare these assets with the
					// rendered template.
					assert!(
						sql.contains(&format!(
							"CREATE POLICY {} ON",
							aidash_server::database::policy::POLICY
						)),
						"{}: {sql}",
						migration.name
					);
				} else if sql.starts_with("ALTER TABLE") {
					// These PostgreSQL JSONB CHECK replacements use reversible SQL
					// assets because the filesystem loader resolves assets in RunSQL.
					let check = match (migration.app_label.as_str(), migration.name.as_str()) {
						("knowledge", "0024_openrouter_embeddings") => {
							Some("semantic_indexes_revision")
						}
						("registry", "0014_openrouter_embeddings") => {
							Some("registry_embedding_config")
						}
						_ => None,
					};
					assert!(
						sql.contains("ADD GENERATED ALWAYS AS IDENTITY")
							|| check.is_some_and(|name| {
								sql.contains(&format!("DROP CONSTRAINT {name};"))
									&& sql.contains(&format!("ADD CONSTRAINT {name} CHECK ("))
							}),
						"{}: {sql}",
						migration.name
					);
				}
			}
		}
	}
	let mut directories = vec![root];
	while let Some(directory) = directories.pop() {
		for entry in std::fs::read_dir(directory).unwrap() {
			let path = entry.unwrap().path();
			if path.is_dir() {
				directories.push(path);
			} else {
				let content = std::fs::read_to_string(&path).unwrap();
				assert!(
					!content.chars().any(|c| matches!(
						c,
						'\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
					)),
					"{}",
					path.display()
				);
			}
		}
	}
}

struct MigrationFixture {
	connection: BackendConnection,
	url: String,
	directory: TempDir,
	_container: ContainerAsync<GenericImage>,
}

#[fixture]
async fn fresh_database(
	#[future] postgres_container: (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String),
	temp_dir: TempDir,
) -> MigrationFixture {
	let (container, pool, _, url) = postgres_container.await;
	pool.close().await;
	MigrationFixture {
		connection: BackendConnection::connect_postgres_with_pool_size(&url, Some(4))
			.await
			.unwrap(),
		url,
		directory: temp_dir,
		_container: container,
	}
}

#[rstest]
#[tokio::test]
async fn makemigrations_writes_only_to_the_requested_directory(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange
	let fixture = fresh_database.await;
	let destination = fixture.directory.path().join("generated migrations");
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	let bin = fixture.directory.path().join("src/bin");
	std::fs::create_dir_all(&bin).unwrap();
	std::fs::copy(
		PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/manage.rs"),
		bin.join("manage.rs"),
	)
	.unwrap();
	command
		.args([
			"makemigrations",
			"execution",
			"--empty",
			"--name",
			"output_probe",
			"--migration-dir",
		])
		.arg(&destination)
		.stdin(Stdio::null());
	// Act
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.expect("empty migration generation must complete")
		.unwrap();
	// Assert
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	let generated = FilesystemSource::new(&destination)
		.all_migrations()
		.await
		.unwrap();
	assert_eq!(generated.len(), 1);
	assert_eq!(generated[0].app_label, "execution");
	assert_eq!(generated[0].name, "0001_output_probe");
	assert!(generated[0].operations.is_empty());
	let original = FilesystemSource::new(fixture.directory.path().join("migrations"))
		.all_migrations()
		.await
		.unwrap();
	assert!(
		original
			.iter()
			.all(|migration| !migration.name.contains("output_probe"))
	);
}

#[rstest]
#[tokio::test]
async fn generation_with_existing_sql_assets_preserves_history_and_dependency(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: generate against the deployed baseline without modifying its files.
	let fixture = fresh_database.await;
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	let history = fixture
		.directory
		.path()
		.join("migrations/execution/0002_tables.rs");
	let original = std::fs::read(&history).unwrap();
	let bin = fixture.directory.path().join("src/bin");
	std::fs::create_dir_all(&bin).unwrap();
	std::fs::copy(
		PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/manage.rs"),
		bin.join("manage.rs"),
	)
	.unwrap();
	command.args([
		"makemigrations",
		"execution",
		"--empty",
		"--name",
		"policy_probe",
	]);
	// Act
	let output = command.output().await.unwrap();
	// Assert
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert_eq!(std::fs::read(history).unwrap(), original);
	let generated = FilesystemSource::new(fixture.directory.path().join("migrations"))
		.get_migration("execution", "0072_policy_probe")
		.await
		.unwrap();
	assert_eq!(
		generated.dependencies,
		vec![(
			"execution".to_owned(),
			"0071_egress_session_requests_state".to_owned()
		)]
	);
	assert!(generated.operations.is_empty());
}

#[rstest]
#[tokio::test]
async fn nonempty_generation_reads_sql_assets_and_replays_only_logical_state(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: change one logical field in a disposable copy of the full history.
	let fixture = fresh_database.await;
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	let root = fixture.directory.path().join("migrations");
	let history = root.join("workspaces/0007_model_state.rs");
	let original = std::fs::read_to_string(&history).unwrap();
	let field = concat!(
		"name: \"artifacts\".to_string(),\n",
		"\t\t\tcolumns: vec![\n",
		"\t\t\t\tColumnDefinition::new(\"content\", FieldType::Jsonb)\n",
		"\t\t\t\t\t.with_not_null(true)",
	);
	assert_eq!(original.matches(field).count(), 1);
	let edited = original.replacen(
		field,
		&field.replace(".with_not_null(true)", ".with_not_null(false)"),
		1,
	);
	std::fs::write(&history, &edited).unwrap();
	let before = FilesystemSource::new(&root).all_migrations().await.unwrap();
	assert_eq!(before.len(), history_size().await);
	let predecessor = build_state_from_files(&FilesystemSource::new(&root))
		.await
		.unwrap();
	assert!(predecessor.find_model_by_table("artifacts").unwrap().fields["content"].nullable);
	let mut input_files = Vec::new();
	let mut directories = vec![root.clone()];
	while let Some(directory) = directories.pop() {
		for entry in std::fs::read_dir(directory).unwrap() {
			let path = entry.unwrap().path();
			if path.is_dir() {
				directories.push(path);
			} else {
				input_files.push((path.clone(), std::fs::read(path).unwrap()));
			}
		}
	}
	let bin = fixture.directory.path().join("src/bin");
	std::fs::create_dir_all(&bin).unwrap();
	std::fs::copy(
		PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/manage.rs"),
		bin.join("manage.rs"),
	)
	.unwrap();
	command
		.args([
			"makemigrations",
			"workspaces",
			"--state-source",
			"files",
			"--name",
			"content_probe",
		])
		.stdin(Stdio::null());

	// Act: save through the native repository against existing include_str! DDL.
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.expect("nonempty migration generation must complete")
		.unwrap();

	// Assert: exactly one field repair is generated, with the existing dependency.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	let after = FilesystemSource::new(&root).all_migrations().await.unwrap();
	assert_eq!(after.len(), before.len() + 1);
	for migration in &before {
		let retained = after
			.iter()
			.find(|item| item.app_label == migration.app_label && item.name == migration.name)
			.unwrap();
		assert_eq!(
			serde_json::to_value(retained).unwrap(),
			serde_json::to_value(migration).unwrap()
		);
	}
	assert_eq!(std::fs::read_to_string(history).unwrap(), edited);
	let generated = after
		.iter()
		.find(|migration| {
			migration.app_label == "workspaces" && migration.name == "0014_content_probe"
		})
		.unwrap();
	assert_eq!(
		generated.dependencies,
		vec![(
			"workspaces".to_owned(),
			"0013_message_file_versions_state".to_owned()
		)]
	);
	assert_eq!(generated.operations.len(), 1);
	let reinhardt::db::migrations::Operation::AlterColumn {
		table,
		column,
		new_definition,
		..
	} = &generated.operations[0]
	else {
		panic!("the generated migration must repair one column");
	};
	assert_eq!(table, "artifacts");
	assert_eq!(column, "content");
	assert!(new_definition.not_null);
	assert_eq!(
		new_definition.type_definition,
		reinhardt::db::migrations::FieldType::Jsonb
	);

	// Replay the same edited history plus the saved repair through the real CLI.
	let mut check = tokio::process::Command::new(env!("CARGO_BIN_EXE_manage"));
	check
		.env_clear()
		.envs(
			command
				.as_std()
				.get_envs()
				.filter_map(|(key, value)| value.map(|value| (key, value))),
		)
		.current_dir(fixture.directory.path())
		.args([
			"makemigrations",
			"--state-source",
			"files",
			"--dry-run",
			"--check",
		])
		.stdin(Stdio::null())
		.kill_on_drop(true);
	let output = tokio::time::timeout(Duration::from_secs(35), check.output())
		.await
		.expect("the saved logical repair must replay")
		.unwrap();
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(String::from_utf8_lossy(&output.stdout).contains("No changes detected"));
	for (path, content) in input_files {
		assert_eq!(std::fs::read(&path).unwrap(), content, "{}", path.display());
	}
	assert!(
		DatabaseMigrationRecorder::new(fixture.connection)
			.get_applied_migrations_if_present()
			.await
			.unwrap()
			.is_empty()
	);
}

impl MigrationFixture {
	async fn migrate(&self) {
		self.migrate_excluding(&[]).await;
	}

	/// The Tenant root record that a Tenant-owned row references.
	async fn tenant_root(&self, name: &str) {
		let insert = Query::insert()
			.into_table(Alias::new("tenants"))
			.columns(["name", "personal", "created_at", "created_by_kind"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(name))
					.expr(Expr::value(false))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::value("deployment_administrator"))
					.to_owned(),
			)
			.on_conflict(
				reinhardt::query::OnConflict::column(Alias::new("name"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder);
		self.connection.execute(&insert, vec![]).await.unwrap();
		let node = aidash_domain::nodes::NodeName::derived(name);
		for sql in [
			Query::insert()
				.into_table(Alias::new("node_names"))
				.columns(["name", "tenant", "created_at"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(node.as_str()))
						.expr(Expr::value(name))
						.expr(Expr::cust("clock_timestamp()"))
						.to_owned(),
				)
				.on_conflict(
					reinhardt::query::OnConflict::column(Alias::new("name"))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
			Query::insert()
				.into_table(Alias::new("nodes"))
				.columns(
					[
						"tenant",
						"name",
						"label",
						"is_default",
						"created_at",
						"created_by_kind",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(name))
						.expr(Expr::value(node.as_str()))
						.expr(Expr::value("Default"))
						.expr(Expr::value(true))
						.expr(Expr::cust("clock_timestamp()"))
						.expr(Expr::value("deployment_administrator"))
						.to_owned(),
				)
				.on_conflict(
					reinhardt::query::OnConflict::columns([
						Alias::new("tenant"),
						Alias::new("name"),
					])
					.do_nothing()
					.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		] {
			self.connection.execute(&sql, vec![]).await.unwrap();
		}
	}

	/// Applies the history as it was before Issue #243's migrations.
	async fn migrate_before_tenant_registry(&self) {
		self.migrate_excluding(TENANT_REGISTRY_MIGRATIONS).await;
	}

	async fn migrate_excluding(&self, excluded: &[(&str, &str)]) {
		let recorder = DatabaseMigrationRecorder::new(self.connection.clone());
		let before = recorder
			.get_applied_migrations_if_present()
			.await
			.unwrap()
			.len();
		let log_path = self.directory.path().join("migrate.log");
		let log = File::create(&log_path).unwrap();
		let mut command = deployment_command_excluding(&self.url, self.directory.path(), excluded);
		command
			.arg("migrate")
			.stdin(Stdio::null())
			.stdout(log.try_clone().unwrap())
			.stderr(log)
			.kill_on_drop(true);
		let result = tokio::time::timeout(Duration::from_secs(35), command.status()).await;
		let output = std::fs::read_to_string(log_path).unwrap();
		assert!(
			result.is_ok(),
			"manage migrate exceeded its deadline: {output}"
		);
		assert!(
			result.unwrap().unwrap().success(),
			"manage migrate failed: {output}"
		);
		let after = recorder
			.get_applied_migrations_if_present()
			.await
			.unwrap()
			.len();
		assert!(
			after >= before,
			"forward migration must preserve applied history"
		);
		assert!(
			output.contains(&format!(
				"Applied {} migration(s) successfully",
				after - before
			)),
			"manage migrate must report newly applied records, including zero on replay: {output}"
		);
	}
}

#[rstest]
#[case::empty_database(false)]
#[case::applied_database(true)]
#[tokio::test]
async fn preserved_baseline_does_not_generate_table_recreation(
	#[future] fresh_database: MigrationFixture,
	#[case] apply_schema: bool,
) {
	// Arrange: the physical baseline and its ORM state share one native ledger.
	let fixture = fresh_database.await;
	if apply_schema {
		fixture.migrate().await;
	}
	let bin = fixture.directory.path().join("src/bin");
	std::fs::create_dir_all(&bin).unwrap();
	std::fs::copy(
		PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/bin/manage.rs"),
		bin.join("manage.rs"),
	)
	.unwrap();
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["makemigrations", "--check", "--dry-run"]);
	// Act: use the shipped command, including installed-app model scope.
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert: neither built-in auth tables nor existing Aidash tables are proposed.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(String::from_utf8_lossy(&output.stdout).contains("No changes detected"));
	let applied = DatabaseMigrationRecorder::new(fixture.connection)
		.get_applied_migrations_if_present()
		.await
		.unwrap();
	let migrations = FilesystemSource::new(fixture.directory.path().join("migrations"))
		.all_migrations()
		.await
		.unwrap();
	assert_eq!(
		applied.len(),
		if apply_schema { migrations.len() } else { 0 }
	);
	assert_eq!(
		migrations
			.iter()
			.filter(|migration| migration.state_only)
			.count(),
		66
	);
}

#[rstest]
#[tokio::test]
async fn native_migrations_build_a_fresh_schema_and_preserve_records_on_replay(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: the fixture has no legacy tables or applied migration history.
	let fixture = fresh_database.await;
	let source =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"));
	let migrations = source.all_migrations().await.unwrap();
	let recorder = DatabaseMigrationRecorder::new(fixture.connection.clone());
	assert!(
		recorder
			.get_applied_migrations_if_present()
			.await
			.unwrap()
			.is_empty()
	);

	// Act: invoke the actual management binary, then persist through the native ORM.
	fixture.migrate().await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	fixture.tenant_root("acme").await;
	let workspace = Workspace::build()
		.id(Uuid::new_v4())
		.tenant("acme")
		.node(aidash_domain::nodes::NodeName::derived("acme").to_string())
		.title("After native migration")
		.goal("Preserve on replay")
		.state(json!({"keep":true}).into())
		.revision(0)
		.finish();
	Workspace::objects()
		.create_with_conn(&mut connection, &workspace)
		.await
		.unwrap();
	fixture.migrate().await;

	// Assert: every generated app migration was applied exactly once and data survived.
	let applied = recorder.get_applied_migrations().await.unwrap();
	let expected: BTreeSet<_> = migrations
		.iter()
		.map(|m| (m.app_label.as_str(), m.name.as_str()))
		.collect();
	let actual: BTreeSet<_> = applied
		.iter()
		.map(|m| (m.app.as_str(), m.name.as_str()))
		.collect();
	assert_eq!(actual, expected);
	assert_eq!(applied.len(), migrations.len());
	let loaded = Workspace::objects()
		.filter(Workspace::field_id().eq(workspace.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(loaded.title, "After native migration");
	assert_eq!(loaded.state.0, json!({"keep":true}));
}

fn migration_context(url: &str) -> reinhardt::commands::CommandContext {
	let mut context = reinhardt::commands::CommandContext::default();
	context.set_option("database".into(), url.to_owned());
	context.set_option(
		"migrations-dir".into(),
		PathBuf::from(env!("CARGO_MANIFEST_DIR"))
			.join("migrations")
			.to_string_lossy()
			.into_owned(),
	);
	context
}

#[rstest]
#[tokio::test]
async fn preprovisioned_extension_allows_database_scoped_migrations(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: the administrator owns the extension, while the application can
	// create objects only in its database and has no superuser privileges.
	let fixture = fresh_database.await;
	let admin = PgPool::connect(&fixture.url).await.unwrap();
	let role = format!("scoped_{}", Uuid::new_v4().simple());
	sqlx::raw_sql("CREATE EXTENSION pg_jsonschema WITH SCHEMA public; CREATE EXTENSION vector; CREATE EXTENSION pgroonga;")
		.execute(&admin)
		.await
		.unwrap();
	// Role administration has no application-schema builder; fixture ownership
	// and its database are bounded by the Testcontainers guard, including panic.
	sqlx::query(&format!(
		"CREATE ROLE {role} LOGIN PASSWORD 'fixture-scoped-password'"
	))
	.execute(&admin)
	.await
	.unwrap();
	sqlx::query(&format!("GRANT CREATE ON SCHEMA public TO {role}"))
		.execute(&admin)
		.await
		.unwrap();
	let mut url = reqwest::Url::parse(&fixture.url).unwrap();
	url.set_username(&role).unwrap();
	url.set_password(Some("fixture-scoped-password")).unwrap();
	let context = migration_context(url.as_str());
	// Act: run the exact native bootstrap twice under the scoped role.
	aidash_server::bootstrap::migrations::run(&context)
		.await
		.unwrap();
	aidash_server::bootstrap::migrations::run(&context)
		.await
		.unwrap();
	// Assert: replay retains one ledger and the application owns its tables.
	let count: i64 = sqlx::query_scalar("SELECT count(*) FROM reinhardt_migrations")
		.fetch_one(&admin)
		.await
		.unwrap();
	assert_eq!(count, history_size().await as i64);
	let owner: String = sqlx::query_scalar(
		"SELECT tableowner FROM pg_tables WHERE schemaname='public' AND tablename='workspaces'",
	)
	.fetch_one(&admin)
	.await
	.unwrap();
	assert_eq!(owner, role);
	let superuser: bool = sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname=$1")
		.bind(&role)
		.fetch_one(&admin)
		.await
		.unwrap();
	assert!(!superuser);
	// Act: the same restricted role can reverse its own graph, then replay it.
	let mut command = deployment_command(url.as_str(), fixture.directory.path());
	command.args(["migrate", "operations", "zero"]);
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert: an administrator's extension survives without ownership transfer.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(recorded_keys(&fixture.connection).await.is_empty());
	let extension_owner: String = sqlx::query_scalar(
		"SELECT pg_get_userbyid(extowner) FROM pg_extension WHERE extname='pg_jsonschema'",
	)
	.fetch_one(&admin)
	.await
	.unwrap();
	assert_eq!(extension_owner, "aidash");
	aidash_server::bootstrap::migrations::run(&context)
		.await
		.unwrap();
	assert_eq!(
		recorded_keys(&fixture.connection).await.len(),
		history_size().await
	);
}

#[rstest]
#[tokio::test]
async fn concurrent_startups_serialize_the_complete_native_baseline(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: two node startups share a genuinely empty database.
	let fixture = fresh_database.await;
	let context = migration_context(&fixture.url);
	// Act: the application lock includes native ledger creation and every app.
	let (first, second) = tokio::join!(
		aidash_server::bootstrap::migrations::run(&context),
		aidash_server::bootstrap::migrations::run(&context),
	);
	first.unwrap();
	second.unwrap();
	// Assert: no duplicate migration, half-created table, or ledger adoption.
	let applied = DatabaseMigrationRecorder::new(fixture.connection.clone())
		.get_applied_migrations()
		.await
		.unwrap();
	let expected =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.len();
	assert_eq!(applied.len(), expected);
	assert_eq!(
		applied
			.iter()
			.map(|row| (&row.app, &row.name))
			.collect::<BTreeSet<_>>()
			.len(),
		expected
	);
}

#[rstest]
#[case::unknown("unrelated", "0001_initial", "unknown migration history")]
#[case::missing_dependency("identity", "0002_tables", "inconsistent migration history")]
#[tokio::test]
async fn unknown_or_inconsistent_native_history_is_not_adopted(
	#[future] fresh_database: MigrationFixture,
	#[case] app: &str,
	#[case] migration: &str,
	#[case] message: &str,
) {
	// Arrange: preserve the invalid row as evidence; repair is a manual action.
	let fixture = fresh_database.await;
	let recorder = DatabaseMigrationRecorder::new(fixture.connection.clone());
	recorder.ensure_schema_table().await.unwrap();
	recorder.record_applied(app, migration).await.unwrap();
	// Act
	let error = aidash_server::bootstrap::migrations::run(&migration_context(&fixture.url))
		.await
		.unwrap_err();
	// Assert
	assert!(
		matches!(error, aidash_server::Error::Invalid(ref text) if text.contains(message)),
		"{error}"
	);
	let applied = recorder.get_applied_migrations().await.unwrap();
	assert_eq!(applied.len(), 1);
	assert_eq!(applied[0].app, app);
	assert_eq!(applied[0].name, migration);
}

async fn recorded_keys(connection: &BackendConnection) -> BTreeSet<(String, String)> {
	DatabaseMigrationRecorder::new(connection.clone())
		.get_applied_migrations()
		.await
		.unwrap()
		.into_iter()
		.map(|record| (record.app, record.name))
		.collect()
}

#[rstest]
#[tokio::test]
async fn baseline_preserves_a_preinstalled_extension_during_reversal(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: extension ownership belongs to whoever runs the Deployment, before native history.
	let fixture = fresh_database.await;
	let create = reinhardt::db::migrations::Operation::CreateExtension {
		name: "pg_jsonschema".into(),
		if_not_exists: false,
		schema: Some("public".into()),
	}
	.try_to_sql(&SqlDialect::Postgres)
	.unwrap();
	fixture.connection.execute(&create, vec![]).await.unwrap();

	// Act: applying and reversing the graph must leave borrowed infrastructure.
	aidash_server::bootstrap::migrations::run(&migration_context(&fixture.url))
		.await
		.unwrap();
	assert_eq!(
		recorded_keys(&fixture.connection).await.len(),
		history_size().await
	);
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["migrate", "operations", "zero"]);
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();

	// Assert: the native graph reverses without assuming extension ownership.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(recorded_keys(&fixture.connection).await.is_empty());
	let extensions = fixture
		.connection
		.fetch_all(
			&Query::select()
				.column(Alias::new("extname"))
				.from((Alias::new("pg_catalog"), Alias::new("pg_extension")))
				.and_where(Expr::col(Alias::new("extname")).eq("pg_jsonschema"))
				.to_string(PostgresQueryBuilder),
			vec![],
		)
		.await
		.unwrap();
	assert_eq!(extensions.len(), 1);
	assert_eq!(
		extensions[0].get::<String>("extname").unwrap(),
		"pg_jsonschema"
	);
}

#[rstest]
#[tokio::test]
async fn local_database_preparation_uses_and_replays_the_native_history(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: use the copied deployment layout, as the packaged binary does.
	let fixture = fresh_database.await;
	let _layout = deployment_command(&fixture.url, fixture.directory.path());
	let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_local-dev-db"));
	command
		.env_clear()
		.env("PATH", std::env::var_os("PATH").unwrap_or_default())
		.env("AIDASH_BASE_DIR", fixture.directory.path())
		.args([&fixture.url, &fixture.url])
		.kill_on_drop(true);

	// Act: the second preparation must reuse the same applied records.
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();

	// Assert: local setup runs the shared migrator and its extension version check.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	let stdout = String::from_utf8_lossy(&output.stdout);
	assert!(stdout.contains("Prepared local Aidash database 1."));
	assert!(stdout.contains("Prepared local Aidash database 2."));
	assert_eq!(
		recorded_keys(&fixture.connection).await.len(),
		history_size().await
	);
}

#[rstest]
#[case::zero("zero")]
#[case::earlier("0002_tables")]
#[case::snapshot("0006_triggers")]
#[tokio::test]
async fn management_refuses_fake_baseline_rollback_without_changing_data_or_history(
	#[future] fresh_database: MigrationFixture,
	#[case] target: &str,
) {
	// Arrange: the baseline has live data and all model-state snapshots recorded.
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	fixture.tenant_root("acme").await;
	let workspace = Workspace::build()
		.id(Uuid::new_v4())
		.tenant("acme")
		.node(aidash_domain::nodes::NodeName::derived("acme").to_string())
		.title("Retained")
		.goal("Refuse unsafe reversal")
		.state(json!({"keep": true}).into())
		.revision(0)
		.finish();
	Workspace::objects()
		.create_with_conn(&mut lease.handle(), &workspace)
		.await
		.unwrap();
	let before = recorded_keys(&fixture.connection).await;
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["migrate", "marketplace", target, "--fake"]);
	// Act: use the actual management entry point, including the entire preflight.
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert: no state snapshot or physical migration is unrecorded.
	assert!(!output.status.success());
	assert!(
		String::from_utf8_lossy(&output.stderr).contains("baseline reversal cannot be faked"),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert_eq!(recorded_keys(&fixture.connection).await, before);
	let retained = Workspace::objects()
		.filter(Workspace::field_id().eq(workspace.id))
		.get_with_db(&mut lease.handle())
		.await
		.unwrap();
	assert_eq!(retained.title, "Retained");
	assert_eq!(retained.state.0, json!({"keep": true}));
}

#[rstest]
#[tokio::test]
async fn out_of_order_physical_reversal_keeps_the_native_ledger(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let before = recorded_keys(&fixture.connection).await;
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let migration = migrations
		.into_iter()
		.find(|migration| migration.app_label == "registry" && migration.name == "0002_tables")
		.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act
	let error = executor
		.rollback_migrations(&[migration])
		.await
		.unwrap_err();
	// Assert: PostgreSQL refuses dropping a referenced table and atomic rollback
	// retains its ledger entry. The management graph must order dependents first.
	assert!(error.to_string().contains("depend"), "{error}");
	assert_eq!(recorded_keys(&fixture.connection).await, before);
}

#[rstest]
#[tokio::test]
async fn complete_baseline_reverses_and_reapplies_through_the_management_graph(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	assert_baseline_seed_data(&fixture.connection).await;
	let before = recorded_keys(&fixture.connection).await;
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["migrate", "operations", "zero"]);
	// Act
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert: every physical migration and state snapshot is reversed, then the
	// same history can build a fresh schema again without changing its identities.
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(recorded_keys(&fixture.connection).await.is_empty());
	let tables = fixture
		.connection
		.fetch_all(
			&Query::select()
				.column(Alias::new("tablename"))
				.from((Alias::new("pg_catalog"), Alias::new("pg_tables")))
				.and_where(Expr::col(Alias::new("schemaname")).eq("public"))
				.to_string(PostgresQueryBuilder),
			vec![],
		)
		.await
		.unwrap();
	assert_eq!(tables.len(), 1);
	assert_eq!(
		tables[0].get::<String>("tablename").unwrap(),
		"reinhardt_migrations"
	);
	let extensions = fixture
		.connection
		.fetch_all(
			&Query::select()
				.column(Alias::new("extname"))
				.from((Alias::new("pg_catalog"), Alias::new("pg_extension")))
				.and_where(Expr::col(Alias::new("extname")).eq("pg_jsonschema"))
				.to_string(PostgresQueryBuilder),
			vec![],
		)
		.await
		.unwrap();
	assert!(extensions.is_empty());
	fixture.migrate().await;
	assert_eq!(recorded_keys(&fixture.connection).await, before);
	assert_baseline_seed_data(&fixture.connection).await;
}

async fn assert_baseline_seed_data(connection: &BackendConnection) {
	let barrier = connection
		.fetch_all(
			&Query::select()
				.column(Alias::new("singleton"))
				.expr_as(
					Expr::col(Alias::new("transaction_id")).is_null(),
					Alias::new("unlocked"),
				)
				.column(Alias::new("commit_epoch"))
				.from((Alias::new("public"), Alias::new("atomic_gate")))
				.to_string(PostgresQueryBuilder),
			vec![],
		)
		.await
		.unwrap();
	assert_eq!(barrier.len(), 1);
	assert!(barrier[0].get::<bool>("singleton").unwrap());
	assert!(barrier[0].get::<bool>("unlocked").unwrap());
	assert_eq!(barrier[0].get::<i64>("commit_epoch").unwrap(), 0);
	// The baseline seeds the Marketplace compatibility gate, which
	// marketplace/0008 later removes with its table.
	let gate = connection
		.fetch_all(
			&Query::select()
				.expr_as(
					Expr::cust("to_regclass('public.marketplace_gate') IS NULL"),
					Alias::new("absent"),
				)
				.to_string(PostgresQueryBuilder),
			vec![],
		)
		.await
		.unwrap();
	assert_eq!(gate.len(), 1);
	assert!(gate[0].get::<bool>("absent").unwrap());
}

#[rstest]
#[case::nel("\u{85}")]
#[case::ls("\u{2028}")]
#[case::ps("\u{2029}")]
#[case::vertical_tab("\u{b}")]
#[case::form_feed("\u{c}")]
#[case::carriage_return("\r")]
#[tokio::test]
async fn escaped_whitespace_checks_still_reject_blank_content(
	#[future] fresh_database: MigrationFixture,
	#[case] whitespace: &str,
) {
	// Arrange: SQL source uses escapes, while the row contains the actual character.
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	fixture.tenant_root("acme").await;
	let workspace = Workspace::build()
		.id(Uuid::new_v4())
		.tenant("acme")
		.node(aidash_domain::nodes::NodeName::derived("acme").to_string())
		.title(whitespace)
		.goal("Valid goal")
		.state(json!({}).into())
		.revision(0)
		.finish();
	// Act
	let error = Workspace::objects()
		.create_with_conn(&mut lease.handle(), &workspace)
		.await
		.unwrap_err();
	// Assert: retain PostgreSQL's frozen nonblank constraint, not only model validation.
	assert!(error.to_string().contains("workspaces_content"), "{error}");
}

#[rstest]
#[tokio::test]
async fn subsequent_native_migrations_can_rollback_to_the_complete_baseline(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange: generated operations after the baseline retain normal native reversal.
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let before = recorded_keys(&fixture.connection).await;
	std::fs::write(
		fixture
			.directory
			.path()
			.join("migrations/marketplace/0012_reversible_probe.rs"),
		r#"
// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
use reinhardt::db::migrations::FieldType;
pub(super) fn migration() -> Migration {
    Migration::new("0012_reversible_probe", "marketplace")
        .add_dependency("marketplace", "0011_tenant_owned_row_level_security")
        .add_operation(Operation::CreateTable {
            name: "rollback_probe".to_string(),
            columns: vec![ColumnDefinition::new("id", FieldType::Integer).with_primary_key(true)],
            constraints: vec![], without_rowid: None, interleave_in_parent: None, partition: None,
        })
}
"#,
	)
	.unwrap();
	fixture.migrate().await;
	let extended = recorded_keys(&fixture.connection).await;
	assert_eq!(extended.len(), before.len() + 1);
	assert!(extended.contains(&("marketplace".into(), "0012_reversible_probe".into())));
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args([
		"migrate",
		"marketplace",
		"0011_tenant_owned_row_level_security",
	]);
	// Act
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert_eq!(recorded_keys(&fixture.connection).await, before);
	let pool = fixture.connection.into_postgres().unwrap();
	let query = Query::select()
		.column(Alias::new("tablename"))
		.from((Alias::new("pg_catalog"), Alias::new("pg_tables")))
		.and_where(Expr::col(Alias::new("schemaname")).eq("public"))
		.and_where(Expr::col(Alias::new("tablename")).eq("rollback_probe"))
		.to_string(PostgresQueryBuilder);
	let tables: Vec<String> = sqlx::query_scalar(&query).fetch_all(&pool).await.unwrap();
	assert!(
		tables.is_empty(),
		"the reversible migration's table must be removed"
	);
}

async fn contract_accepts(pool: &PgPool, function: &str, values: Vec<serde_json::Value>) -> bool {
	use reinhardt::query::IntoIden;
	let query = Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			Alias::new(function).into_iden(),
			values
				.into_iter()
				.map(|v| match v {
					serde_json::Value::String(text) => Expr::value(text).into(),
					value => Expr::value(value).into(),
				})
				.collect(),
		))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query).fetch_one(pool).await.unwrap()
}

async fn constraint_definition(pool: &PgPool, name: &str) -> String {
	use reinhardt::query::IntoIden;
	let query = Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			Alias::new("pg_get_constraintdef").into_iden(),
			vec![Expr::col("oid").into()],
		))
		.from(Alias::new("pg_constraint"))
		.and_where(Expr::col("conname").eq(name))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&query).fetch_one(pool).await.unwrap()
}

#[rstest]
#[tokio::test]
async fn prompt_cache_migration_admits_new_keys_and_reverses(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |cache: Option<serde_json::Value>| {
		let mut config = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work","projection_version":"ordered"});
		if let Some(mode) = cache {
			config["prompt_cache"] = mode;
		}
		config
	};
	let model = |id: &str, cache_mode: serde_json::Value| {
		let metadata = json!({"id":id,"version":"1.0.0","kind":"model",
			"name":{"en":"Prompt cache fixture"},"description":{"en":"Migration test"},
			"config":{"provider":"openrouter","model_id":"anthropic/model",
				"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
				"modalities":["text"],"cost":{},
				"projection_versions":["legacy","ordered"],"cache_mode":cache_mode}});
		Definition::build()
			.tenant("acme")
			.id(id)
			.version("1.0.0")
			.kind(DefinitionKind::Model)
			.metadata(metadata.into())
			.finish()
	};
	// Act / Assert: the Agent contract admits prompt_cache off and explicit only.
	for (cache, accepted) in [
		(None, true),
		(Some(json!(null)), true),
		(Some(json!("off")), true),
		(Some(json!("explicit")), true),
		(Some(json!("automatic")), false),
		(Some(json!(true)), false),
	] {
		assert_eq!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(cache.clone())]
			)
			.await,
			accepted,
			"{cache:?}"
		);
	}
	// The catalog edit extends the current allowlist, keeping earlier additions.
	let upgraded = constraint_definition(&pool, "registry_model_config").await;
	for key in ["cache_mode", "projection_versions", "provider_credential"] {
		assert!(upgraded.contains(&format!("'{key}'::text")), "{upgraded}");
	}
	assert!(
		upgraded.contains("'projection_versions'::text, 'cache_mode'::text]"),
		"{upgraded}"
	);
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let migration = migrations
		.into_iter()
		.find(|m| m.app_label == "registry" && m.name == "0018_prompt_cache")
		.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Assert: the reverse restores the 0017 contracts.
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(Some(json!("explicit")))]
		)
		.await
	);
	assert!(contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![agent(None)]).await);
	assert_eq!(
		constraint_definition(&pool, "registry_model_config").await,
		upgraded.replace(", 'cache_mode'::text", "")
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	assert_eq!(
		constraint_definition(&pool, "registry_model_config").await,
		upgraded
	);
	// Act / Assert: the model constraints admit declared cache modes only.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	for (id, mode) in [
		("none-model", "none"),
		("automatic-model", "automatic"),
		("explicit-model", "explicit"),
	] {
		Definition::objects()
			.create_with_conn(&mut connection, &model(id, json!(mode)))
			.await
			.unwrap();
	}
	for (id, mode) in [("bad-model", json!("always")), ("typed-model", json!(true))] {
		let error = Definition::objects()
			.create_with_conn(&mut connection, &model(id, mode))
			.await
			.unwrap_err();
		let database = error.database_error().expect("database constraint error");
		assert_eq!(
			database.constraint(),
			Some("registry_model_cache_mode"),
			"{error}"
		);
	}
	// Act / Assert: a stored key refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("registry 0018_prompt_cache cannot be reversed"),
		"{error}"
	);
	assert_eq!(
		constraint_definition(&pool, "registry_model_config").await,
		upgraded
	);
}

#[rstest]
#[tokio::test]
async fn tool_parallelism_migration_reverses_to_deferred_exposure_and_refuses_stored_fields(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |extra: serde_json::Value| {
		let mut config = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work","projection_version":"ordered","prompt_cache":"explicit"});
		config
			.as_object_mut()
			.unwrap()
			.extend(extra.as_object().unwrap().clone());
		config
	};
	let parallel = json!({"tool_parallelism":2});
	let deferred = json!({"exposure":{"version":"deferred@1"}});
	// The 0021 bodies keep the 0019 deferred exposure contract beside the ceiling.
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(
				json!({"tool_parallelism":2,"exposure":{"version":"deferred@1"}})
			)]
		)
		.await
	);
	let query = Query::select()
		.expr(Expr::cust(
			"pg_get_functiondef('public.guard_installation_config()'::regprocedure)",
		))
		.to_string(PostgresQueryBuilder);
	let upgraded: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
	assert!(
		upgraded.contains("'cluster','max_steps','exposure','tool_parallelism']::text[]"),
		"{upgraded}"
	);
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let migration = migrations
		.into_iter()
		.find(|m| m.app_label == "registry" && m.name == "0021_tool_parallelism")
		.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act: reverse with no stored parallelism fields.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Assert: the 0019_deferred_exposure Agent contract and guard are restored.
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(deferred)]
		)
		.await
	);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(parallel.clone())]
		)
		.await
	);
	let restored: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
	assert_eq!(restored, upgraded.replace(",'tool_parallelism'", ""));
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	let reapplied: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
	assert_eq!(reapplied, upgraded);
	// Arrange: a registered Agent declaring a Tool Parallelism ceiling.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Parallel fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{}}});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("model")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(model.into())
				.finish(),
		)
		.await
		.unwrap();
	// A missing optional restriction on an earlier Binding must not hide a
	// later concurrency declaration. No Tool Parallelism ceiling is present,
	// so these rollbacks exercise the nested restriction guard itself.
	for concurrency in [None, Some("sequential"), Some("shared_read")] {
		let id = format!("binding-{}", concurrency.unwrap_or("unrestricted"));
		let target = |id: &str| json!({"registry_deployment":"aidash://migration","id":id,"version":"1.0.0"});
		let mut config = agent(json!({"bindings":[
			{"kind":"tool","target":target("first"),"alias":"first"},
			{"kind":"tool","target":target("second"),"alias":"second","narrow":{}},
			{"kind":"tool","target":target("third"),"alias":"third"}
		]}));
		if let Some(concurrency) = concurrency {
			config["bindings"][2]["narrow"] = json!({"concurrency":concurrency});
		}
		let metadata = json!({"id":id,"version":"1.0.0","kind":"agent",
			"name":{"en":"Binding guard"},"description":{"en":"Migration test"},
			"config":config});
		Definition::objects()
			.create_with_conn(
				&mut connection,
				&Definition::build()
					.tenant("acme")
					.id(id.clone())
					.version("1.0.0")
					.kind(DefinitionKind::Agent)
					.metadata(metadata.into())
					.finish(),
			)
			.await
			.unwrap();
		let result = executor
			.rollback_migrations(std::slice::from_ref(&migration))
			.await;
		if concurrency.is_some() {
			let error = result.unwrap_err();
			assert!(
				error.to_string().contains(
					"definitions, packages or installations declare tool_parallelism or narrow.concurrency"
				),
				"{error}"
			);
			assert!(
				contract_accepts(
					&pool,
					"aidash_agent_bindings_is_valid",
					vec![agent(parallel.clone())]
				)
				.await,
				"the denied rollback must retain the upgraded contract"
			);
		} else {
			result.unwrap();
			executor
				.apply_migrations(std::slice::from_ref(&migration))
				.await
				.unwrap();
		}
		Definition::objects()
			.filter(Definition::field_id().eq(id))
			.delete_with_conn(&mut connection)
			.await
			.unwrap();
	}

	let metadata = json!({"id":"parallel","version":"1.0.0","kind":"agent",
		"name":{"en":"Parallel"},"description":{"en":"Migration test"},"config":agent(parallel.clone())});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("parallel")
				.version("1.0.0")
				.kind(DefinitionKind::Agent)
				.metadata(metadata.into())
				.finish(),
		)
		.await
		.unwrap();
	// Act / Assert: the stored field refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("registry 0021_tool_parallelism cannot be reversed"),
		"{error}"
	);
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(parallel)]
		)
		.await
	);
}

#[rstest]
#[tokio::test]
async fn context_policy_migration_validates_policies_and_refuses_rollback_while_stored(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |policy: serde_json::Value| {
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work",
			"tool_parallelism":2,"exposure":{"version":"deferred@1"},"context_policy":policy})
	};
	let summarized = json!({"version":"context-recovery/1","preserve_recent":8,
		"overflow_retries":4,"overflow_shrink":0.5,
		"summary":{"model":{"id":"model","version":"1.0.0"},"max_tokens":256,"call_budget":64}});
	// Assert: the contract mirrors ContextPolicy::validate, beside the 0021 keys.
	for valid in [
		summarized.clone(),
		json!({"version":"context-recovery/1"}),
		json!({"version":"context-recovery/1","overflow_retries":0,"overflow_shrink":0.9,"summary":null}),
		json!(null),
	] {
		assert!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(valid.clone())]
			)
			.await,
			"{valid}"
		);
	}
	let summary = |field: serde_json::Value| {
		let mut policy = summarized.clone();
		policy["summary"]
			.as_object_mut()
			.unwrap()
			.extend(field.as_object().unwrap().clone());
		policy
	};
	for invalid in [
		json!("context-recovery/1"),
		json!({"version":"context-recovery/2"}),
		json!({"preserve_recent":8}),
		json!({"version":"context-recovery/1","unknown":true}),
		json!({"version":"context-recovery/1","preserve_recent":5}),
		json!({"version":"context-recovery/1","preserve_recent":1001}),
		json!({"version":"context-recovery/1","preserve_recent":6.5}),
		json!({"version":"context-recovery/1","preserve_recent":null}),
		json!({"version":"context-recovery/1","overflow_retries":5}),
		json!({"version":"context-recovery/1","overflow_shrink":0.95}),
		json!({"version":"context-recovery/1","overflow_shrink":0.4}),
		json!({"version":"context-recovery/1","overflow_shrink":"0.75"}),
		json!({"version":"context-recovery/1","summary":{"max_tokens":256}}),
		summary(json!({"max_tokens":255})),
		summary(json!({"max_tokens":4097})),
		summary(json!({"call_budget":0})),
		summary(json!({"call_budget":65})),
		summary(json!({"model":{"id":"","version":"1.0.0"}})),
		summary(json!({"fallback":true})),
	] {
		assert!(
			!contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(invalid.clone())]
			)
			.await,
			"{invalid}"
		);
	}
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0022_context_policy")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without stored policies the reverse restores the 0021 contract.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	let mut plain = agent(json!(null));
	plain.as_object_mut().unwrap().remove("context_policy");
	assert!(contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![plain]).await);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(summarized.clone())]
		)
		.await
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Arrange: the registry write path admits a valid policy and rejects an invalid one.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Context fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{}}});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("model")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(model.into())
				.finish(),
		)
		.await
		.unwrap();
	let definition = |id: &str, policy: serde_json::Value| {
		Definition::build()
			.tenant("acme")
			.id(id)
			.version("1.0.0")
			.kind(DefinitionKind::Agent)
			.metadata(
				json!({"id":id,"version":"1.0.0","kind":"agent",
					"name":{"en":"Context policy"},"description":{"en":"Migration test"},
					"config":agent(policy)})
				.into(),
			)
			.finish()
	};
	assert!(
		Definition::objects()
			.create_with_conn(
				&mut connection,
				&definition(
					"unbounded",
					json!({"version":"context-recovery/1","overflow_retries":5})
				),
			)
			.await
			.is_err(),
		"the database contract must reject an invalid Context Policy"
	);
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&definition("recovering", summarized.clone()),
		)
		.await
		.unwrap();
	// Act / Assert: the stored policy refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains(
			"registry 0022_context_policy cannot be reversed: definitions, packages or installations declare context_policy"
		),
		"{error}"
	);
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(summarized)]
		)
		.await,
		"the denied rollback must retain the upgraded contract"
	);
}

#[rstest]
#[tokio::test]
async fn completion_policy_migration_validates_policies_and_refuses_rollback_while_stored(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |policy: serde_json::Value| {
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work",
			"context_policy":{"version":"context-recovery/1"},"completion":policy})
	};
	let check = |field: serde_json::Value| {
		let mut check = json!({"name":"tests","capability":"shell","command":"cargo test"});
		check
			.as_object_mut()
			.unwrap()
			.extend(field.as_object().unwrap().clone());
		json!({"version":"checks/1","checks":[check]})
	};
	let full = json!({"version":"checks/1","repair_allowance":16,"verification_seconds":3600,
		"exception":true,"checks":[
			{"name":"build","capability":"shell","command":"make","timeout_seconds":600,"when":"always"},
			{"name":"unit-tests_2","capability":"shell","command":"make test","timeout_seconds":null,"when":"content_changed"}]});
	// Assert: the contract mirrors CompletionPolicy::validate beside the 0022 keys.
	for valid in [full.clone(), check(json!({})), json!(null)] {
		assert!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(valid.clone())]
			)
			.await,
			"{valid}"
		);
	}
	for invalid in [
		json!("checks/1"),
		json!({"version":"checks/2","checks":[{"name":"t","capability":"shell","command":"x"}]}),
		json!({"version":"checks/1","checks":[]}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"repair_allowance":17}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"verification_seconds":0}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"exception":"yes"}),
		// CompletionPolicy has no Option for these, so an explicit null is not
		// an omission.
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"repair_allowance":null}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"verification_seconds":null}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"exception":null}),
		json!({"version":"checks/1","checks":null}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"}],"unknown":true}),
		json!({"version":"checks/1","checks":[{"name":"t","capability":"shell","command":"x"},{"name":"t","capability":"shell","command":"y"}]}),
		check(json!({"capability":"code_interpreter"})),
		check(json!({"name":"Tests"})),
		check(json!({"name":"-tests"})),
		check(json!({"command":"  "})),
		// Blank to Rust's `trim`, not just to one-argument `btrim`.
		check(json!({"command":"\t\n\u{3000}"})),
		check(json!({"command":"x".repeat(65_537)})),
		check(json!({"timeout_seconds":0})),
		check(json!({"timeout_seconds":601})),
		check(json!({"when":"sometimes"})),
		check(json!({"when":null})),
		check(json!({"arguments":"chosen by the model"})),
	] {
		assert!(
			!contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(invalid.clone())]
			)
			.await,
			"{invalid}"
		);
	}
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0029_completion_policy")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without stored policies the reverse restores the 0022 contract.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	let mut plain = agent(json!(null));
	plain.as_object_mut().unwrap().remove("completion");
	assert!(contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![plain]).await);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(full.clone())]
		)
		.await
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Arrange: the registry write path stores a valid policy.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Completion fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{}}});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("model")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(model.into())
				.finish(),
		)
		.await
		.unwrap();
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("verified")
				.version("1.0.0")
				.kind(DefinitionKind::Agent)
				.metadata(
					json!({"id":"verified","version":"1.0.0","kind":"agent",
						"name":{"en":"Completion policy"},"description":{"en":"Migration test"},
						"config":agent(full.clone())})
					.into(),
				)
				.finish(),
		)
		.await
		.unwrap();
	// Act / Assert: the stored policy refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains(
			"registry 0029_completion_policy cannot be reversed: Agent definitions declare completion"
		),
		"{error}"
	);
	assert!(
		contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![agent(full)]).await,
		"the denied rollback must retain the upgraded contract"
	);
}

#[rstest]
#[tokio::test]
async fn agent_plugins_migration_validates_references_and_refuses_rollback_while_stored(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let digest = format!("sha256:{}", "ab01".repeat(16));
	let plugin = |field: serde_json::Value| {
		let mut plugin = json!({"digest":digest,"name":"tools","source":"https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567",
			"skills":["review"],"servers":{},"findings":[]});
		plugin
			.as_object_mut()
			.unwrap()
			.extend(field.as_object().unwrap().clone());
		plugin
	};
	let agent = |instructions: &str, plugins: serde_json::Value| {
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":instructions,
			"agent_plugins":plugins})
	};
	// Assert: the contract mirrors AgentBindings::validate_agent_plugins' shape.
	for valid in [
		json!([]),
		json!([plugin(json!({}))]),
		json!([plugin(json!({"version":"1.2.0","license":"MIT"}))]),
		json!(
			(0..8)
				.map(|n| plugin(json!({"name":format!("tools-{n}")})))
				.collect::<Vec<_>>()
		),
	] {
		assert!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent("Work", valid.clone())]
			)
			.await,
			"{valid}"
		);
	}
	for invalid in [
		json!(null),
		json!("plugins"),
		json!({"digest":digest}),
		json!([null]),
		json!([plugin(json!({"digest":"sha256:ABCD"}))]),
		json!([plugin(
			json!({"digest":format!("sha256:{}", "ab".repeat(31))})
		)]),
		json!([plugin(
			json!({"digest":format!("sha256:{}", "AB".repeat(32))})
		)]),
		json!([plugin(json!({"digest":7}))]),
		json!([plugin(json!({"name":7}))]),
		json!([plugin(json!({"source":null}))]),
		json!([plugin(json!({"skills":{}}))]),
		json!([plugin(json!({"findings":"none"}))]),
		json!([plugin(json!({"servers":[]}))]),
		json!(
			(0..9)
				.map(|n| plugin(json!({"name":format!("tools-{n}")})))
				.collect::<Vec<_>>()
		),
	] {
		assert!(
			!contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent("Work", invalid.clone())]
			)
			.await,
			"{invalid}"
		);
	}
	// Assert: a plugin with Skills stands in for instructions, one without does not.
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent("", json!([plugin(json!({}))]))]
		)
		.await
	);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent("", json!([plugin(json!({"skills":[]}))]))]
		)
		.await
	);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent("", json!([]))]
		)
		.await
	);
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0041_agent_plugins_definition")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without stored references the reverse restores the 0029 contract.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![
				json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work"})
			]
		)
		.await
	);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent("Work", json!([plugin(json!({}))]))]
		)
		.await
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Arrange: the registry write path stores a valid reference.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Plugin fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{}}});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("model")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(model.into())
				.finish(),
		)
		.await
		.unwrap();
	let referenced = agent("", json!([plugin(json!({}))]));
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("tooled")
				.version("1.0.0")
				.kind(DefinitionKind::Agent)
				.metadata(
					json!({"id":"tooled","version":"1.0.0","kind":"agent",
						"name":{"en":"Plugin agent"},"description":{"en":"Migration test"},
						"config":referenced.clone()})
					.into(),
				)
				.finish(),
		)
		.await
		.unwrap();
	// Act / Assert: the stored reference refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains(
			"registry 0041_agent_plugins_definition cannot be reversed: Agent definitions declare agent_plugins"
		),
		"{error}"
	);
	assert!(
		contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![referenced]).await,
		"the denied rollback must retain the upgraded contract"
	);
}

#[rstest]
#[tokio::test]
async fn agent_plugin_settings_migration_validates_the_key_and_refuses_rollback_while_stored(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |settings: serde_json::Value| {
		json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work",
			"agent_plugin_settings":settings})
	};
	let digest = format!("sha256:{}", "ab01".repeat(16));
	// Generated Agents disable implicit compaction. The replacement validator
	// must preserve that key and the current Decider Binding restrictions.
	let mut compaction = json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},
		"instructions":"Work","remove_default":["compaction"]});
	let mut deferred_compaction = compaction.clone();
	deferred_compaction["exposure"] = json!({"version":"deferred@1"});
	compaction["bindings"] = json!([{"kind":"decider",
		"target":{"registry_deployment":"aidash://local","id":"decider","version":"1.0.0"},
		"narrow":{"decision":{"threshold":"3fd0000000000000","preserve_recent":8,
			"forbid_apply":true,"max_calls_per_run":2}}}]);
	let compaction_agents = [deferred_compaction, compaction];
	for config in &compaction_agents {
		let mut with_settings = config.clone();
		with_settings["agent_plugin_settings"] = json!({"acme":{"docs":{"tools":["search"]}}});
		assert!(
			contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![with_settings]).await,
			"Agent Plugin settings must retain the compaction contract: {config}"
		);
	}
	// Assert: the contract mirrors PluginSettings' shape: plugin to server to settings.
	for valid in [
		json!({}),
		json!({"acme":{}}),
		json!({"acme":{"docs":{}}}),
		json!({"acme":{"docs":{"tools":["search","fetch"]}}}),
		json!({"acme":{"docs":{"tools":[]}}}),
		json!({"acme":{"docs":{"tools":null,"exposure":null}}}),
		json!({"acme":{"docs":{"exposure":"eager"},"wiki":{"exposure":"deferred"}}}),
		json!({"acme":{"docs":{"read_only":[]}}}),
		json!({"acme":{"docs":{"tools":["search"],"exposure":"eager",
			"read_only":[{"name":"search","schema_digest":digest}]}}}),
		json!({"acme":{"docs":{}},"globex":{"api":{"exposure":"eager"}}}),
	] {
		assert!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(valid.clone())]
			)
			.await,
			"{valid}"
		);
	}
	for invalid in [
		json!(null),
		json!([]),
		json!("settings"),
		json!({"acme":null}),
		json!({"acme":[]}),
		json!({"acme":"docs"}),
		json!({"acme":{"docs":null}}),
		json!({"acme":{"docs":[]}}),
		json!({"acme":{"docs":{"tool":["search"]}}}),
		json!({"acme":{"docs":{"tools":"search"}}}),
		json!({"acme":{"docs":{"tools":{}}}}),
		json!({"acme":{"docs":{"tools":[1]}}}),
		json!({"acme":{"docs":{"exposure":{"mode":"eager"}}}}),
		json!({"acme":{"docs":{"exposure":"lazy"}}}),
		json!({"acme":{"docs":{"exposure":true}}}),
		json!({"acme":{"docs":{"read_only":null}}}),
		json!({"acme":{"docs":{"read_only":{}}}}),
		json!({"acme":{"docs":{"read_only":["search"]}}}),
		json!({"acme":{"docs":{"read_only":[{"name":"search"}]}}}),
		json!({"acme":{"docs":{"read_only":[{"schema_digest":digest}]}}}),
		json!({"acme":{"docs":{"read_only":[{"name":1,"schema_digest":digest}]}}}),
		json!({"acme":{"docs":{"read_only":[{"name":"search","schema_digest":7}]}}}),
		json!({"acme":{"docs":{"read_only":[{"name":"search","schema_digest":digest,"note":"x"}]}}}),
	] {
		assert!(
			!contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(invalid.clone())]
			)
			.await,
			"{invalid}"
		);
	}
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0045_agent_plugin_settings")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without stored settings the reverse restores the 0042 contract.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	for config in &compaction_agents {
		assert!(
			contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![config.clone()]
			)
			.await,
			"rollback must retain the compaction contract: {config}"
		);
	}
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(json!({"acme":{"docs":{}}}))]
		)
		.await
	);
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![
				json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work","agent_plugins":[]})
			]
		)
		.await
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Arrange: the registry write path stores a definition with settings.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Plugin fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{}}});
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("model")
				.version("1.0.0")
				.kind(DefinitionKind::Model)
				.metadata(model.into())
				.finish(),
		)
		.await
		.unwrap();
	let stored = agent(json!({"acme":{"docs":{"tools":["search"]}}}));
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("tooled")
				.version("1.0.0")
				.kind(DefinitionKind::Agent)
				.metadata(
					json!({"id":"tooled","version":"1.0.0","kind":"agent",
						"name":{"en":"Plugin agent"},"description":{"en":"Migration test"},
						"config":stored.clone()})
					.into(),
				)
				.finish(),
		)
		.await
		.unwrap();
	// Act / Assert: the stored settings refuse the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains(
			"registry 0045_agent_plugin_settings cannot be reversed: Agent definitions declare agent_plugin_settings"
		),
		"{error}"
	);
	assert!(
		contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![stored]).await,
		"the denied rollback must retain the upgraded contract"
	);
}

#[rstest]
#[tokio::test]
async fn agent_plugin_copies_key_by_tenant_and_digest_and_check_their_documents(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::models::AgentPluginCopy;
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	fixture.tenant_root("globex").await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let digest = format!("sha256:{}", "cd23".repeat(16));
	let copy = |tenant: &str, digest: &str, plugin: serde_json::Value, files: serde_json::Value| {
		AgentPluginCopy::build()
			.tenant(tenant)
			.digest(digest)
			.name("tools")
			.source("https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567")
			.plugin(plugin.into())
			.files(files.into())
			.denied(json!([]).into())
			.entitlement_expires_at(None)
			.finish()
	};
	let plugin = json!({"digest":digest,"manifest":{"name":"tools"}});
	// Act
	let created = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy("acme", &digest, plugin.clone(), json!([])),
		)
		.await;
	let other_tenant = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy("globex", &digest, plugin.clone(), json!([])),
		)
		.await;
	let duplicate = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy("acme", &digest, plugin.clone(), json!([])),
		)
		.await;
	let unknown_tenant = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy("initech", &digest, plugin.clone(), json!([])),
		)
		.await;
	let bad_digest = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy(
				"acme",
				"sha256:ABCD",
				json!({"digest":"sha256:ABCD"}),
				json!([]),
			),
		)
		.await;
	let other_digest = format!("sha256:{}", "ef45".repeat(16));
	let mismatched = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy("acme", &other_digest, plugin.clone(), json!([])),
		)
		.await;
	let files_not_array = AgentPluginCopy::objects()
		.create_with_conn(
			&mut connection,
			&copy(
				"acme",
				&other_digest,
				json!({"digest":other_digest}),
				json!({}),
			),
		)
		.await;
	// Assert
	assert!(created.is_ok(), "{created:?}");
	assert!(other_tenant.is_ok(), "{other_tenant:?}");
	assert!(duplicate.is_err(), "a Tenant keeps one copy per digest");
	assert!(
		unknown_tenant.is_err(),
		"the Tenant must have a root record"
	);
	assert!(
		bad_digest.is_err(),
		"the digest is sha256: and lowercase hex"
	);
	assert!(
		mismatched.is_err(),
		"the plugin document carries the key's digest"
	);
	assert!(files_not_array.is_err(), "files are an array");
}

#[rstest]
#[tokio::test]
async fn degraded_context_migration_admits_only_workspace_opt_ins_and_refuses_rollback_while_stored(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let source = |source: serde_json::Value| json!({"schema_version":1,"source":source});
	let accepts = |config: serde_json::Value, kind: &'static str| {
		let pool = pool.clone();
		async move {
			contract_accepts(&pool, "aidash_context_is_valid", vec![config, json!(kind)]).await
		}
	};
	let degrade = source(json!({"adapter":"workspace_retrieval","on_unavailable":"degrade"}));
	// Assert: only workspace_retrieval may declare pause or degrade (ADR 0059).
	for valid in [
		source(json!({"adapter":"workspace_retrieval"})),
		source(json!({"adapter":"workspace_retrieval","on_unavailable":"pause"})),
		degrade.clone(),
	] {
		assert!(accepts(valid.clone(), "source").await, "{valid}");
	}
	for (invalid, kind) in [
		(
			source(json!({"adapter":"workspace_retrieval","on_unavailable":"skip"})),
			"source",
		),
		(
			source(json!({"adapter":"workspace_retrieval","on_unavailable":null})),
			"source",
		),
		(
			source(json!({"adapter":"workspace_retrieval","on_unavailable":true})),
			"source",
		),
		(
			source(json!({"adapter":"semantic_memory","on_unavailable":"degrade"})),
			"memory",
		),
	] {
		assert!(!accepts(invalid.clone(), kind).await, "{invalid}");
	}
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0032_degraded_context")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without stored opt-ins the reverse restores the 0011 validator.
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	assert!(accepts(source(json!({"adapter":"workspace_retrieval"})), "source").await);
	assert!(!accepts(degrade.clone(), "source").await);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Arrange: the registry write path stores an opted-in source.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	Definition::objects()
		.create_with_conn(
			&mut connection,
			&Definition::build()
				.tenant("acme")
				.id("workspace")
				.version("1.0.0")
				.kind(DefinitionKind::Source)
				.metadata(
					json!({"id":"workspace","version":"1.0.0","kind":"source",
						"name":{"en":"Workspace"},"description":{"en":"Migration test"},
						"config":degrade.clone()})
					.into(),
				)
				.finish(),
		)
		.await
		.unwrap();
	// Act / Assert: the stored opt-in refuses the rollback before any DDL runs.
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("registry 0032_degraded_context cannot be reversed: 1 Definitions"),
		"{error}"
	);
	assert!(
		accepts(degrade, "source").await,
		"the denied rollback must retain the upgraded validator"
	);
}

#[rstest]
#[tokio::test]
async fn model_streaming_migration_composes_with_deferred_exposure_guard(
	#[future] fresh_database: MigrationFixture,
) {
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let query = Query::select()
		.expr(Expr::cust(
			"pg_get_functiondef('public.guard_installation_config()'::regprocedure)",
		))
		.to_string(PostgresQueryBuilder);
	// The later 0021_tool_parallelism appends `tool_parallelism` to this allowlist.
	let exposure = "'remove_default','cluster','max_steps','exposure','tool_parallelism']::text[]";
	let upgraded: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
	assert!(upgraded.contains(exposure), "{upgraded}");
	assert!(
		upgraded.contains("'streaming','stream_stall_timeout_secs'"),
		"{upgraded}"
	);
	let migration =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap()
			.into_iter()
			.find(|m| m.app_label == "registry" && m.name == "0020_model_streaming_config")
			.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	// Assert: rollback withdraws only the streaming checks, keeping 0019's guard.
	let restored: String = sqlx::query_scalar(&query).fetch_one(&pool).await.unwrap();
	assert!(restored.contains(exposure), "{restored}");
	assert!(!restored.contains("stream"), "{restored}");
}

#[rstest]
#[tokio::test]
async fn binding_memory_merge_preserves_native_packages_and_restores_legacy_descriptors(
	#[future] fresh_database: MigrationFixture,
) {
	use sha2::{Digest, Sha256};
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let reference = |id: &str| json!({"id":id,"version":"1.0.0"});
	let zero = json!({"input_per_million":0,"output_per_million":0});
	let provider = json!({"engine":"hindsight_rust","policy":{
            "extraction":reference("fixture-model"),"derivation":reference("fixture-model"),"reflection":reference("fixture-model"),"embedding":reference("native-embedding"),"reranker":reference("native-reranker"),"tokenizer":reference("native-tokenizer"),
            "prices":{"extraction":zero,"derivation":zero,"reflection":zero,"embedding":zero,"reranker":zero},
            "retention":{"unit_max_age_days":null,"candidate_days":7,"history_days":30,"history_versions":16,"model_result_days":7,"backup_days":7,"purge_after_seconds":60,"purge_batch":32,"max_unit_records":128,"max_model_operations":1024},
            "bounds":{"max_unit_bytes":8192,"max_input_bytes":8192,"max_units":16,"max_candidates":8,"max_entities":8,"max_evidence":8,"max_links":8,"max_graph_hops":3,"max_graph_visits":32,"max_results":4,"max_context_tokens":8192,"max_model_calls":4,"max_model_tokens":8192,"max_cost_micros":10000,"max_retries":2,"max_call_seconds":30},
            "semantic_link_min_similarity_millionths":700000,"learn_from_runs":false,"maintain_observations":false,"refresh_mental_models":false}});
	let source = json!({"scope":"participant","memory":reference("native"),"max_tokens":4096});
	for (kind, config) in [("memory", provider.clone()), ("source", source.clone())] {
		let id = format!("native-{kind}-package");
		let manifest = json!({"entity":{"id":id,"version":"1.0.0","kind":kind,"name":{"en":"Native"},"description":{"en":"Fixture"},"schema":{},"config":config},"author":"fixture","permissions":[],"dependencies":[]});
		assert!(
			contract_accepts(
				&pool,
				"aidash_binding_package_is_valid",
				vec![manifest.clone(), json!(id), json!("1.0.0")]
			)
			.await
		);
		// The legacy table predates Issue #243, which removed its ORM model.
		let text = serde_json::to_string(&manifest).unwrap();
		sqlx::query(
			"INSERT INTO packages (id, version, manifest, digest, manifest_source) VALUES ($1, '1.0.0', $2, $3, $4)",
		)
		.bind(&id)
		.bind(&manifest)
		.bind(format!("sha256:{:x}", Sha256::digest(text.as_bytes())))
		.bind(&text)
		.execute(&pool)
		.await
		.unwrap();
	}
	let mut invalid = provider.clone();
	invalid["policy"]["unexpected"] = json!(true);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_native_context_is_valid",
			vec![invalid, json!("memory")]
		)
		.await
	);
	let mut invalid = source;
	invalid["max_tokens"] = json!(0);
	assert!(
		!contract_accepts(
			&pool,
			"aidash_native_context_is_valid",
			vec![invalid, json!("source")]
		)
		.await
	);
	let current = serde_json::to_value(
		aidash_domain::tool::providers::core_descriptor("aidash://fixture", "memory_mutate")
			.unwrap(),
	)
	.unwrap();
	let mut legacy = current.clone();
	legacy["operation"] = json!("memory_write");
	legacy["default_alias"] = json!("memory_write");
	assert!(contract_accepts(&pool, "aidash_descriptor_is_valid", vec![current.clone()]).await);
	assert!(!contract_accepts(&pool, "aidash_descriptor_is_valid", vec![legacy.clone()]).await);
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let migration = migrations
		.into_iter()
		.find(|m| m.app_label == "registry" && m.name == "0015_binding_memory_merge")
		.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	assert!(contract_accepts(&pool, "aidash_descriptor_is_valid", vec![legacy.clone()]).await);
	assert!(!contract_accepts(&pool, "aidash_descriptor_is_valid", vec![current.clone()]).await);
	executor.apply_migrations(&[migration]).await.unwrap();
	assert!(contract_accepts(&pool, "aidash_descriptor_is_valid", vec![current]).await);
	assert!(!contract_accepts(&pool, "aidash_descriptor_is_valid", vec![legacy]).await);
}

#[rstest]
#[case::retention(
	"0027_memory_retention_lookup",
	"memory_unit_retention",
	"idx_memory_unit_retention_bank_id_dormant_policy_pinned",
	"USING btree (bank_id, dormant_policy, pinned)"
)]
#[case::run_reads_by_unit(
	"0029_memory_run_read_lookup",
	"memory_run_reads",
	"idx_memory_run_reads_unit_id_run_id_revision",
	"USING btree (unit_id, run_id, revision)"
)]
#[tokio::test]
async fn memory_lookup_index_upgrades_and_reverses_without_rewriting_tables(
	#[future] fresh_database: MigrationFixture,
	#[case] name: &str,
	#[case] table: &str,
	#[case] index: &str,
	#[case] definition: &str,
) {
	let fixture = fresh_database.await;
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let lookup = migrations
		.iter()
		.find(|migration| migration.app_label == "knowledge" && migration.name == name)
		.unwrap()
		.clone();
	// The baseline omits the lookup and every migration that depends on it, so
	// later knowledge migrations never run ahead of their missing parent.
	let mut excluded = vec![(lookup.app_label.to_string(), lookup.name.to_string())];
	loop {
		let descendants: Vec<_> = migrations
			.iter()
			.filter(|migration| {
				let key = (migration.app_label.to_string(), migration.name.to_string());
				!excluded.contains(&key)
					&& migration
						.dependencies
						.iter()
						.any(|(app, name)| excluded.contains(&(app.to_string(), name.to_string())))
			})
			.map(|migration| (migration.app_label.to_string(), migration.name.to_string()))
			.collect();
		if descendants.is_empty() {
			break;
		}
		excluded.extend(descendants);
	}
	let baseline: Vec<_> = migrations
		.into_iter()
		.filter(|migration| {
			!excluded.contains(&(migration.app_label.to_string(), migration.name.to_string()))
		})
		.collect();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	executor.apply_migrations(&baseline).await.unwrap();
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let query = Query::select()
		.column(Alias::new("indexdef"))
		.from((Alias::new("pg_catalog"), Alias::new("pg_indexes")))
		.and_where(Expr::col("schemaname").eq("public"))
		.and_where(Expr::col("tablename").eq(table))
		.and_where(Expr::col("indexname").eq(index))
		.to_string(PostgresQueryBuilder);
	assert!(
		sqlx::query_scalar::<_, String>(&query)
			.fetch_all(&pool)
			.await
			.unwrap()
			.is_empty()
	);
	executor
		.apply_migrations(std::slice::from_ref(&lookup))
		.await
		.unwrap();
	let created: Vec<String> = sqlx::query_scalar(&query).fetch_all(&pool).await.unwrap();
	assert_eq!(created.len(), 1);
	assert!(created[0].contains(definition), "{}", created[0]);
	executor
		.rollback_migrations(std::slice::from_ref(&lookup))
		.await
		.unwrap();
	assert!(
		sqlx::query_scalar::<_, String>(&query)
			.fetch_all(&pool)
			.await
			.unwrap()
			.is_empty()
	);
	executor.apply_migrations(&[lookup]).await.unwrap();
	let reapplied: Vec<String> = sqlx::query_scalar(&query).fetch_all(&pool).await.unwrap();
	assert_eq!(reapplied, created);
}

#[rstest]
#[tokio::test]
async fn deferred_exposure_contract_rejects_explicit_null_budgets(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let agent = |exposure: serde_json::Value| json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Assist.","exposure":exposure});
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![agent(json!({"version":"deferred@1","schema_bytes":4096}))]
		)
		.await
	);
	for budget in ["metadata_bytes", "schema_bytes", "skill_bytes"] {
		let mut exposure = json!({"version":"deferred@1"});
		exposure[budget] = serde_json::Value::Null;
		assert!(
			!contract_accepts(
				&pool,
				"aidash_agent_bindings_is_valid",
				vec![agent(exposure)]
			)
			.await,
			"{budget}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn deferred_exposure_rollback_is_refused_while_an_agent_uses_exposure(
	#[future] fresh_database: MigrationFixture,
) {
	use aidash_server::apps::registry::{models::Definition, services::states::DefinitionKind};
	// Arrange
	let fixture = fresh_database.await;
	fixture.migrate_before_tenant_registry().await;
	fixture.tenant_root("acme").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let deferred = json!({"schema_version":1,"model":{"id":"exposure-model","version":"1.0.0"},
		"instructions":"Assist.","exposure":{"version":"deferred@1"}});
	let migrations =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
			.all_migrations()
			.await
			.unwrap();
	let registry = |name: &str| {
		migrations
			.iter()
			.find(|m| m.app_label == "registry" && m.name == name)
			.cloned()
			.unwrap()
	};
	let migration = registry("0019_deferred_exposure");
	// The later 0021_tool_parallelism migration rewrites the same contracts.
	let later = registry("0021_tool_parallelism");
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act / Assert: without exposure data the reverse restores the 0018 contract.
	executor
		.rollback_migrations(std::slice::from_ref(&later))
		.await
		.unwrap();
	executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	assert!(
		!contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![deferred.clone()]
		)
		.await
	);
	executor
		.apply_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap();
	executor
		.apply_migrations(std::slice::from_ref(&later))
		.await
		.unwrap();
	assert!(
		contract_accepts(
			&pool,
			"aidash_agent_bindings_is_valid",
			vec![deferred.clone()]
		)
		.await
	);
	// Arrange: register an Agent with a deferred Exposure policy.
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	let model = json!({"id":"exposure-model","version":"1.0.0","kind":"model",
		"name":{"en":"Exposure fixture"},"description":{"en":"Migration test"},
		"config":{"provider":"openrouter","model_id":"anthropic/model",
			"endpoint":"http://127.0.0.1:1/v1","context_window":32768,"max_output_tokens":4096,
			"modalities":["text"],"cost":{},
			"projection_versions":["legacy","ordered"],"cache_mode":"none"}});
	let agent = json!({"id":"exposure-agent","version":"1.0.0","kind":"agent",
		"name":{"en":"Exposure agent"},"description":{"en":"Migration test"},
		"config":deferred.clone()});
	for (id, kind, metadata) in [
		("exposure-model", DefinitionKind::Model, model),
		("exposure-agent", DefinitionKind::Agent, agent),
	] {
		let definition = Definition::build()
			.tenant("acme")
			.id(id)
			.version("1.0.0")
			.kind(kind)
			.metadata(metadata.into())
			.finish();
		Definition::objects()
			.create_with_conn(&mut connection, &definition)
			.await
			.unwrap();
	}
	// Act
	executor
		.rollback_migrations(std::slice::from_ref(&later))
		.await
		.unwrap();
	let error = executor
		.rollback_migrations(std::slice::from_ref(&migration))
		.await
		.unwrap_err();
	// Assert: the guard refuses before any contract is restored.
	assert!(
		error
			.to_string()
			.contains("registry 0019_deferred_exposure cannot be reversed: 1 Definitions"),
		"{error}"
	);
	assert!(contract_accepts(&pool, "aidash_agent_bindings_is_valid", vec![deferred]).await);
}

#[rstest]
#[tokio::test]
async fn context_sources_migration_accepts_exact_native_policies_and_rejects_mismatched_operations(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let accepts = |config: serde_json::Value| {
		let pool = pool.clone();
		async move {
			contract_accepts(
				&pool,
				"aidash_context_is_valid",
				vec![config, json!("source")],
			)
			.await
		}
	};
	let declaration = json!({"provider":"aidash.native-context/1","operation":"workspace_retrieval","tools":{"read":[],"write":[],"detail":null},"pin":{"policy":"always"},"query":"accepted-inputs-first@1"});
	let configured = json!({"schema_version":1,"source":{"adapter":"workspace_retrieval"},"context_source":declaration});
	assert!(
		accepts(json!({"schema_version":1,"source":{"adapter":"workspace_observation"}})).await
	);
	assert!(accepts(configured.clone()).await);
	let tool = json!({"registry_deployment":"aidash://test","id":"shared-tool","version":"1.0.0"});
	let mut shared_roles = configured.clone();
	shared_roles["context_source"]["tools"]["read"] = json!([tool]);
	shared_roles["context_source"]["tools"]["write"] = json!([tool]);
	assert!(
		accepts(shared_roles.clone()).await,
		"one exact Tool may serve both roles"
	);
	for role in ["read", "write"] {
		let mut duplicate = shared_roles.clone();
		duplicate["context_source"]["tools"][role] = json!([tool, tool]);
		assert!(
			!accepts(duplicate).await,
			"duplicates within {role} must be rejected"
		);
	}
	for (field, value) in [
		("provider", json!("unavailable.provider")),
		("operation", json!("skills")),
		("query", json!("unknown@1")),
		("required_home", json!("true")),
	] {
		let mut invalid = configured.clone();
		invalid["context_source"][field] = value;
		assert!(!accepts(invalid.clone()).await, "{invalid}");
	}
	let mut unknown = configured;
	unknown["context_source"]["unknown"] = json!(true);
	assert!(!accepts(unknown).await);
}

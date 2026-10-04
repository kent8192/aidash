//! Fresh-schema verification for the native Reinhardt migration command.
#[path = "support/deployment.rs"]
mod deployment;
use aidash_server::apps::workspaces::models::Workspace;
use deployment::deployment_command;
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::migrations::{DatabaseMigrationRecorder, FilesystemSource, MigrationSource};
use reinhardt::db::orm::Model;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
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

impl MigrationFixture {
	async fn migrate(&self) {
		let recorder = DatabaseMigrationRecorder::new(self.connection.clone());
		let before = recorder
			.get_applied_migrations_if_present()
			.await
			.unwrap()
			.len();
		let log_path = self.directory.path().join("migrate.log");
		let log = File::create(&log_path).unwrap();
		let mut command = deployment_command(&self.url, self.directory.path());
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
	// Arrange: the SQL baseline and its ORM state share one native ledger.
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
		8
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
	let workspace = Workspace::build()
		.id(Uuid::new_v4())
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
	// Arrange: preserve the invalid row as evidence; repair is an operator action.
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
#[case::zero("zero", false)]
#[case::earlier("0002_tables", false)]
#[case::snapshot("0006_triggers", false)]
#[case::fake_zero("zero", true)]
#[tokio::test]
async fn management_refuses_baseline_rollback_without_changing_data_or_history(
	#[future] fresh_database: MigrationFixture,
	#[case] target: &str,
	#[case] fake: bool,
) {
	// Arrange: the baseline has live data and all model-state snapshots recorded.
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let workspace = Workspace::build()
		.id(Uuid::new_v4())
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
	command.args(["migrate", "marketplace", target]);
	if fake {
		command.arg("--fake");
	}
	// Act: use the actual management entry point, including the entire preflight.
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	// Assert: no state snapshot or physical migration is unrecorded.
	assert!(!output.status.success());
	assert!(
		String::from_utf8_lossy(&output.stderr).contains("frozen baseline is forward-only"),
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
async fn direct_physical_baseline_reversal_keeps_the_native_ledger(
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
		.find(|migration| migration.app_label == "marketplace" && migration.name == "0002_tables")
		.unwrap();
	let mut executor =
		reinhardt::db::migrations::DatabaseMigrationExecutor::new(fixture.connection.clone());
	// Act
	let error = executor
		.rollback_migrations(&[migration])
		.await
		.unwrap_err();
	// Assert: the reverse guard executes before the executor removes this record.
	assert!(
		error
			.to_string()
			.contains("frozen baseline is forward-only"),
		"{error}"
	);
	assert_eq!(recorded_keys(&fixture.connection).await, before);
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
			.join("migrations/marketplace/0008_reversible_probe.rs"),
		r#"
// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
use reinhardt::db::migrations::FieldType;
pub(super) fn migration() -> Migration {
    Migration::new("0008_reversible_probe", "marketplace")
        .add_dependency("marketplace", "0007_model_state")
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
	assert!(extended.contains(&("marketplace".into(), "0008_reversible_probe".into())));
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["migrate", "marketplace", "0007_model_state"]);
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
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
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

//! Fresh-schema verification for the native Reinhardt migration command.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/deployment.rs"]
mod deployment;
use aidash_server::apps::workspaces::models::Workspace;
use deployment::{deployment_command, deployment_command_excluding};
use reinhardt::db::backends::DatabaseConnection as BackendConnection;
use reinhardt::db::migrations::{DatabaseMigrationRecorder, FilesystemSource, MigrationSource};
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

async fn history_size() -> usize {
	FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
		.all_migrations()
		.await
		.unwrap()
		.len()
}

#[rstest]
#[tokio::test]
async fn baseline_separates_state_from_physical_schema() {
	let source =
		FilesystemSource::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"));
	let migrations = source.all_migrations().await.unwrap();
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
	assert!(graph.detect_conflicts().is_empty());
	let apps = [
		"execution",
		"federation",
		"identity",
		"knowledge",
		"marketplace",
		"operations",
		"registry",
		"workspaces",
	];
	for app in apps {
		let find = |name: &str| {
			migrations
				.iter()
				.find(|m| m.app_label == app && m.name == name)
				.unwrap()
		};
		let state = find("0001_state");
		assert!(state.state_only && !state.database_only);
		assert!(
			state
				.operations
				.iter()
				.all(|op| !matches!(op, reinhardt::db::migrations::Operation::RunSQL { .. }))
		);
		let tables = find("0002_tables");
		assert!(tables.database_only && !tables.state_only);
		let references = find("0003_references");
		assert!(references.database_only && !references.state_only);
		for dependency in apps {
			assert!(
				references
					.dependencies
					.iter()
					.any(|(label, name)| label == dependency && name == "0002_tables")
			);
		}
	}
	assert!(
		migrations
			.iter()
			.any(|m| m.app_label == "operations" && m.name == "0000_environment")
	);
	let progress = migrations
		.iter()
		.filter(|m| m.state_only)
		.flat_map(|m| &m.operations)
		.find_map(|operation| match operation {
			reinhardt::db::migrations::Operation::CreateTable { name, columns, .. }
				if name == "inference_progress" =>
			{
				columns.iter().find(|column| column.name == "seq")
			}
			_ => None,
		})
		.expect("inference progress state");
	assert!(progress.primary_key && !progress.auto_increment);
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

	async fn scratch_space(&self, tenant: &str) -> Uuid {
		let id = Uuid::new_v4();
		let node = aidash_domain::nodes::NodeName::derived(tenant).to_string();
		let insert = Query::insert()
			.into_table(Alias::new("scratch_spaces"))
			.columns(
				[
					"id",
					"tenant",
					"node",
					"name",
					"creator",
					"state",
					"created_at",
					"last_used_at",
				]
				.map(Alias::new),
			)
			.values_panic::<_, reinhardt::query::Value>([
				id.into(),
				tenant.into(),
				node.into(),
				"Fixture".into(),
				"alice".into(),
				"active".into(),
				chrono::Utc::now().into(),
				chrono::Utc::now().into(),
			])
			.to_string(PostgresQueryBuilder);
		self.connection.execute(&insert, vec![]).await.unwrap();
		id
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
	let log_path = fixture.directory.path().join("schema-check.log");
	let log = File::create(&log_path).unwrap();
	command.stdout(log.try_clone().unwrap()).stderr(log);
	// Act: use the shipped command, including installed-app model scope.
	let started = std::time::Instant::now();
	let mut child = command.spawn().unwrap();
	eprintln!(
		"schema-check child={} log={}",
		child.id().unwrap(),
		log_path.display()
	);
	let result = tokio::time::timeout(Duration::from_secs(35), child.wait()).await;
	eprintln!("schema-check elapsed={:?}", started.elapsed());
	let output = std::fs::read_to_string(log_path).unwrap();
	assert!(
		result.is_ok(),
		"manage makemigrations exceeded its deadline: {output}"
	);
	// Assert: neither built-in auth tables nor existing Aidash tables are proposed.
	assert!(
		result.unwrap().unwrap().success(),
		"manage makemigrations failed: {output}"
	);
	assert!(output.contains("No changes detected"));
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
	assert_baseline_seed_data(&fixture.connection).await;
	let lease = DatabaseConnectionLease::register(fixture.connection.clone()).unwrap();
	let mut connection = lease.handle();
	fixture.tenant_root("acme").await;
	let scratch_space_id = fixture.scratch_space("acme").await;
	let workspace = Workspace::build()
		.scratch_space(scratch_space_id)
		.created_by("alice")
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
	// The retired Marketplace compatibility gate is absent from the baseline.
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
	let scratch_space_id = fixture.scratch_space("acme").await;
	let workspace = Workspace::build()
		.scratch_space(scratch_space_id)
		.created_by("alice")
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

async fn recorded_keys(connection: &BackendConnection) -> BTreeSet<(String, String)> {
	DatabaseMigrationRecorder::new(connection.clone())
		.get_applied_migrations()
		.await
		.unwrap()
		.into_iter()
		.map(|r| (r.app, r.name))
		.collect()
}

#[rstest]
#[case::zero("zero")]
#[case::earlier("0002_tables")]
#[case::state("0001_state")]
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
	let scratch_space_id = fixture.scratch_space("acme").await;
	let workspace = Workspace::build()
		.scratch_space(scratch_space_id)
		.created_by("alice")
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
async fn physical_baseline_rejects_reversal_and_preserves_the_ledger(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	fixture.migrate().await;
	let before = recorded_keys(&fixture.connection).await;
	let mut command = deployment_command(&fixture.url, fixture.directory.path());
	command.args(["migrate", "marketplace", "zero"]);
	let output = tokio::time::timeout(Duration::from_secs(35), command.output())
		.await
		.unwrap()
		.unwrap();
	assert!(!output.status.success());
	assert!(
		String::from_utf8_lossy(&output.stderr).contains("baseline is irreversible"),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert_eq!(recorded_keys(&fixture.connection).await, before);
}

#[rstest]
#[tokio::test]
async fn scope_egress_rows_are_tenant_bound_and_purged_with_the_node(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	fixture.migrate().await;
	fixture.tenant_root("acme").await;
	fixture.tenant_root("beta").await;
	let pool = fixture.connection.clone().into_postgres().unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let other_node = aidash_domain::nodes::NodeName::derived("beta").to_string();
	let indexes: Vec<String> = sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE tablename IN ('egress_scope_connections', 'egress_scope_events')").fetch_all(&pool).await.unwrap();
	assert!(
		indexes
			.iter()
			.any(|definition| definition.contains("egress_scope_connections")
				&& definition.contains("(tenant, node, scope_id)"))
	);
	assert!(
		indexes
			.iter()
			.any(|definition| definition.contains("egress_scope_events")
				&& definition.contains("(tenant, node, scope_id)"))
	);
	assert!(
		indexes
			.iter()
			.any(|definition| definition.contains("egress_scope_events")
				&& definition.contains("delivered_at IS NULL"))
	);
	let scope = Uuid::new_v4();
	sqlx::query("INSERT INTO egress_scope_state (scope_id, tenant, node, suspended, window_started_at, connections_in_window) VALUES ($1, 'acme', $2, false, now(), 0)")
		.bind(scope).bind(&node).execute(&pool).await.unwrap();
	sqlx::query("INSERT INTO egress_scope_daily (id, tenant, node, scope_id, day, service, sent_bytes, received_bytes) VALUES ($1, 'acme', $2, $3, CURRENT_DATE::text, '', 1, 2)")
		.bind(Uuid::new_v4()).bind(&node).bind(scope).execute(&pool).await.unwrap();
	let foreign = sqlx::query("INSERT INTO egress_scope_daily (id, tenant, node, scope_id, day, service, sent_bytes, received_bytes) VALUES ($1, 'beta', $2, $3, CURRENT_DATE::text, '', 1, 2)")
		.bind(Uuid::new_v4()).bind(&other_node).bind(scope).execute(&pool).await;
	assert!(
		foreign.is_err(),
		"a different Tenant cannot refer to the scope"
	);
	sqlx::query("INSERT INTO egress_scope_connections (id, tenant, node, scope_id, host, port, sent_bytes, received_bytes, duration_ms, outcome, created_at) VALUES ($1, 'acme', $2, $3, 'example.com', 443, 1, 2, 3, 'completed', now())")
		.bind(Uuid::new_v4()).bind(&node).bind(scope).execute(&pool).await.unwrap();
	sqlx::query("INSERT INTO egress_scope_connections (id, tenant, node, scope_id, host, port, sent_bytes, received_bytes, duration_ms, outcome, created_at) VALUES ($1, 'acme', $2, $3, 'example.com', 0, 0, 0, 0, 'invalid_proxy_target', now())")
		.bind(Uuid::new_v4()).bind(&node).bind(scope).execute(&pool).await.unwrap();
	let invalid_completed = sqlx::query("INSERT INTO egress_scope_connections (id, tenant, node, scope_id, host, port, sent_bytes, received_bytes, duration_ms, outcome, created_at) VALUES ($1, 'acme', $2, $3, 'example.com', 0, 0, 0, 0, 'completed', now())")
		.bind(Uuid::new_v4()).bind(&node).bind(scope).execute(&pool).await;
	assert!(
		invalid_completed.is_err(),
		"port zero cannot represent a completed connection"
	);
	sqlx::query("INSERT INTO egress_scope_events (id, tenant, node, scope_id, kind, created_at) VALUES ($1, 'acme', $2, $3, 'suspended', now())")
		.bind(Uuid::new_v4()).bind(&node).bind(scope).execute(&pool).await.unwrap();
	sqlx::query("DELETE FROM nodes WHERE tenant = 'acme' AND name = $1")
		.bind(&node)
		.execute(&pool)
		.await
		.unwrap();
	for table in [
		"egress_scope_state",
		"egress_scope_daily",
		"egress_scope_connections",
		"egress_scope_events",
	] {
		let count: i64 =
			sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE scope_id = $1"))
				.bind(scope)
				.fetch_one(&pool)
				.await
				.unwrap();
		assert_eq!(count, 0, "{table} survives Node purge");
	}
}

/// PostgreSQL owns immutable grace deadlines and one open deletion per Node;
/// a restored deletion remains historical and does not block a later deletion.
#[rstest]
#[tokio::test]
async fn node_deletion_history_preserves_deadlines_and_permanent_ownership(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	// This owner contract concerns the applied schema; command-process launch
	// remains covered separately by the existing migration lifecycle tests.
	aidash_server::bootstrap::migrations::run(&migration_context(&fixture.url))
		.await
		.unwrap();
	fixture.tenant_root("acme").await;
	fixture.tenant_root("sibling").await;
	let pool = PgPool::connect(&fixture.url).await.unwrap();
	// Grace is elapsed UTC time even when the connection crosses a local DST
	// change. A one-day deadline remains exactly 24 hours, rather than 25.
	let mut session = pool.acquire().await.unwrap();
	sqlx::query("SET TIME ZONE 'America/New_York'")
		.execute(&mut *session)
		.await
		.unwrap();
	let exact_day = sqlx::query("INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,purge_failures) VALUES ($1,'sibling',$2,'2026-11-01 00:00:00+00','deployment_administrator','2026-11-02 00:00:00+00',0)")
        .bind(Uuid::new_v4()).bind(aidash_domain::nodes::NodeName::derived("sibling").to_string()).execute(&mut *session).await;
	assert!(
		exact_day.is_ok(),
		"one elapsed UTC day must fit in any session timezone: {exact_day:?}"
	);
	drop(session);
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let insert = "INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,purge_failures) VALUES ($1,$2,$3,CURRENT_TIMESTAMP,'deployment_administrator',CURRENT_TIMESTAMP + INTERVAL '30 days',0)";
	let first = Uuid::new_v4();
	sqlx::query(insert)
		.bind(first)
		.bind("acme")
		.bind(&node)
		.execute(&pool)
		.await
		.unwrap();
	assert!(
		sqlx::query(insert)
			.bind(Uuid::new_v4())
			.bind("acme")
			.bind(&node)
			.execute(&pool)
			.await
			.is_err(),
		"one unresolved deletion per Node"
	);
	assert!(
		sqlx::query(insert)
			.bind(Uuid::new_v4())
			.bind("sibling")
			.bind(&node)
			.execute(&pool)
			.await
			.is_err(),
		"a Tenant must own the permanent Node name"
	);
	assert!(
		sqlx::query(
			"UPDATE node_deletions SET purge_after=purge_after + INTERVAL '1 day' WHERE id=$1"
		)
		.bind(first)
		.execute(&pool)
		.await
		.is_err(),
		"progress cannot extend the fixed grace period"
	);
	sqlx::query("UPDATE node_deletions SET restored_at=CURRENT_TIMESTAMP, restored_by_kind='deployment_administrator' WHERE id=$1").bind(first).execute(&pool).await.unwrap();
	sqlx::query(insert)
		.bind(Uuid::new_v4())
		.bind("acme")
		.bind(&node)
		.execute(&pool)
		.await
		.unwrap();
	sqlx::query("DELETE FROM nodes WHERE tenant='acme' AND name=$1")
		.bind(&node)
		.execute(&pool)
		.await
		.unwrap();
	let retained: i64 =
		sqlx::query_scalar("SELECT count(*) FROM node_deletions WHERE tenant='acme' AND node=$1")
			.bind(&node)
			.fetch_one(&pool)
			.await
			.unwrap();
	assert_eq!(retained, 2, "purging a Node preserves its deletion history");
	let names: i64 =
		sqlx::query_scalar("SELECT count(*) FROM node_names WHERE tenant='acme' AND name=$1")
			.bind(&node)
			.fetch_one(&pool)
			.await
			.unwrap();
	assert_eq!(names, 1, "the name claim is permanent");
	pool.close().await;
}

/// The audit record keeps its permanent Tenant and Node ownership.
#[rstest]
#[tokio::test]
async fn node_administration_audit_preserves_permanent_tenant_ownership(
	#[future] fresh_database: MigrationFixture,
) {
	let fixture = fresh_database.await;
	aidash_server::bootstrap::migrations::run(&migration_context(&fixture.url))
		.await
		.unwrap();
	fixture.tenant_root("acme").await;
	fixture.tenant_root("sibling").await;
	let pool = PgPool::connect(&fixture.url).await.unwrap();
	let name = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let insert = "INSERT INTO dashboard_administration_history (id,occurred_at,action,actor_kind,tenant,node) VALUES ($1,clock_timestamp(),'node.purge','deployment_administrator',$2,$3)";
	let wrong_owner = sqlx::query(insert)
		.bind(Uuid::new_v4())
		.bind("sibling")
		.bind(&name)
		.execute(&pool)
		.await
		.unwrap_err();
	assert_eq!(
		wrong_owner.as_database_error().unwrap().code().as_deref(),
		Some("23503")
	);
	let no_owner = sqlx::query(insert)
		.bind(Uuid::new_v4())
		.bind(Option::<String>::None)
		.bind(&name)
		.execute(&pool)
		.await
		.unwrap_err();
	assert_eq!(
		no_owner.as_database_error().unwrap().code().as_deref(),
		Some("23514")
	);
	let id = Uuid::new_v4();
	sqlx::query(insert)
		.bind(id)
		.bind("acme")
		.bind(&name)
		.execute(&pool)
		.await
		.unwrap();
	sqlx::query("DELETE FROM nodes WHERE tenant='acme' AND name=$1")
		.bind(&name)
		.execute(&pool)
		.await
		.unwrap();
	let retained: (String, String) =
		sqlx::query_as("SELECT tenant,node FROM dashboard_administration_history WHERE id=$1")
			.bind(id)
			.fetch_one(&pool)
			.await
			.unwrap();
	assert_eq!(retained, ("acme".to_owned(), name.clone()));
	let permanent_claim = sqlx::query("DELETE FROM node_names WHERE name=$1")
		.bind(name)
		.execute(&pool)
		.await
		.unwrap_err();
	assert_eq!(
		permanent_claim
			.as_database_error()
			.unwrap()
			.code()
			.as_deref(),
		Some("23503")
	);
	pool.close().await;
}

//! Serialize the native migration engine and validate its persisted history.
use crate::{Error, Result};
use reinhardt::commands::{BaseCommand, CommandContext, MigrateCommand};
use reinhardt::db::{
	backends::DatabaseConnection,
	migrations::{DatabaseMigrationRecorder, FilesystemSource, MigrationSource},
	orm::execution::convert_values,
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use std::{collections::HashSet, path::PathBuf};

/// Leave the migration ledger read-only to every role but its owner.
///
/// The owner's default privileges grant the server role DML on every table it
/// creates (#262), including the ledger, which `require_current` trusts before
/// a server starts. A server login that could write the ledger could record a
/// migration that never ran. Reinhardt Query cannot express REVOKE or iterate a
/// table's grantees, so this is raw SQL; `SELECT` stays for the startup check.
const SEAL_LEDGER: &str = "DO $$ DECLARE grantee text; BEGIN \
	IF to_regclass('reinhardt_migrations') IS NULL THEN RETURN; END IF; \
	FOR grantee IN SELECT DISTINCT CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE quote_ident(g.rolname) END \
	FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a LEFT JOIN pg_roles g ON g.oid = a.grantee \
	WHERE c.oid = 'reinhardt_migrations'::regclass AND a.grantee <> c.relowner \
	AND a.privilege_type <> 'SELECT' LOOP \
	EXECUTE format('REVOKE INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER \
	ON reinhardt_migrations FROM %s', grantee); END LOOP; END $$";

/// All entry points hold the same transaction-scoped lock for the complete run,
/// including creation of the native ledger. Dropping the transaction on errors
/// or cancellation releases it; no session lock can leak into the pool.
#[allow(clippy::disallowed_methods)] // The owner applies migrations; no Tenant data is read.
pub async fn run(context: &CommandContext) -> Result<()> {
	let url = context
		.option("database")
		.ok_or_else(|| Error::Invalid("migration database is required".into()))?;
	let connection = DatabaseConnection::connect_postgres_or_create(url)
		.await
		.map_err(|_| {
			Error::External("Failed to connect to PostgreSQL migration database".into())
		})?;
	let mut schema_lock = connection.begin().await?;
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(71003203_i64).into()],
		))
		.build(PostgresQueryBuilder);
	schema_lock.execute(&sql, convert_values(values)).await?;

	let directory = context
		.option("migrations-dir")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"));
	let migrations = FilesystemSource::new(&directory)
		.all_migrations()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	if migrations.is_empty() {
		return Err(Error::Invalid(
			"Aidash migration sources are missing".into(),
		));
	}
	let recorder = DatabaseMigrationRecorder::new(connection.clone());
	let applied = recorder
		.get_applied_migrations_if_present()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	let known: HashSet<_> = migrations
		.iter()
		.map(|migration| (migration.app_label.as_str(), migration.name.as_str()))
		.collect();
	let recorded: HashSet<_> = applied
		.iter()
		.map(|migration| (migration.app.as_str(), migration.name.as_str()))
		.collect();
	if !recorded.is_subset(&known) {
		return Err(Error::Invalid(
			"unknown migration history; restore a supported database before continuing".into(),
		));
	}
	for migration in &migrations {
		if recorded.contains(&(migration.app_label.as_str(), migration.name.as_str()))
			&& migration
				.dependencies
				.iter()
				.any(|(app, name)| !recorded.contains(&(app.as_str(), name.as_str())))
		{
			return Err(Error::Invalid(
				"inconsistent migration history: an applied dependency is missing".into(),
			));
		}
	}
	if !recorded.contains(&("operations", "0000_environment"))
		&& (context.has_option("fake") || context.has_option("fake-initial"))
	{
		return Err(Error::Invalid(
			"the Aidash empty-database baseline cannot be faked".into(),
		));
	}
	if !context.has_option("plan") && context.has_option("fake") {
		refuse_fake_baseline_rollback(context, &applied, &migrations)?;
	}
	let mut native = context.clone();
	native.set_option(
		"migrations-dir".into(),
		directory.to_string_lossy().into_owned(),
	);
	let migrated = MigrateCommand
		.execute(&native)
		.await
		.map_err(|error| Error::External(error.to_string()));
	// Each migration commits on its own, so a failed run still leaves the
	// ledger, and the records of the migrations it applied, behind: seal it
	// either way. A run killed outright is sealed by the next `migrate`. The
	// migration's own error takes precedence over the seal's and the commit's.
	let sealed = if context.has_option("plan") {
		Ok(())
	} else {
		schema_lock
			.execute(SEAL_LEDGER, Vec::new())
			.await
			.map(|_| ())
	};
	let committed = schema_lock.commit().await;
	migrated?;
	sealed?;
	committed?;
	Ok(())
}

/// Confirm that every native migration is applied, without changing the schema.
///
/// Servers, workers and maintenance commands log in as a role that cannot
/// change the schema (#262), so only `aidash migrate`, run as the owner of
/// every table, applies migrations.
#[allow(clippy::disallowed_methods)] // Reads only the migration ledger.
pub async fn require_current(url: &str, directory: &std::path::Path) -> Result<()> {
	let connection = DatabaseConnection::connect_postgres(url)
		.await
		.map_err(|_| Error::External("Failed to connect to PostgreSQL".into()))?;
	let migrations = FilesystemSource::new(directory)
		.all_migrations()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	let applied = DatabaseMigrationRecorder::new(connection.clone())
		.get_applied_migrations_if_present()
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	connection
		.into_postgres()
		.expect("a PostgreSQL connection")
		.close()
		.await;
	let known: HashSet<_> = migrations
		.iter()
		.map(|migration| (migration.app_label.as_str(), migration.name.as_str()))
		.collect();
	let recorded: HashSet<_> = applied
		.iter()
		.map(|migration| (migration.app.as_str(), migration.name.as_str()))
		.collect();
	if migrations.is_empty() || !recorded.is_subset(&known) {
		return Err(Error::Invalid(
			"unknown migration history; restore a supported database before continuing".into(),
		));
	}
	let pending = known.difference(&recorded).count();
	if pending > 0 {
		return Err(Error::Invalid(format!(
			"{pending} migrations are not applied; run `aidash migrate` as the database owner first"
		)));
	}
	Ok(())
}

/// Fake reversal would leave physical objects behind after removing baseline
/// records, making the next fresh-schema check and replay unsafe.
fn refuse_fake_baseline_rollback(
	context: &CommandContext,
	applied: &[reinhardt::db::migrations::recorder::MigrationRecord],
	migrations: &[reinhardt::db::migrations::Migration],
) -> Result<()> {
	let (Some(app), Some(target)) = (context.arg(0), context.arg(1)) else {
		return Ok(());
	};
	let records: Vec<_> = applied
		.iter()
		.filter(|record| record.app.as_str() == app.as_str())
		.collect();
	let first = if target == "zero" {
		Some(0)
	} else {
		records
			.iter()
			.position(|record| record.name.as_str() == target.as_str())
			.map(|position| position + 1)
	};
	let Some(first) = first else {
		return Ok(());
	};
	let keys: HashSet<_> = migrations
		.iter()
		.filter(|migration| migration.database_only || migration.state_only)
		.map(|migration| (migration.app_label.clone(), migration.name.clone()))
		.collect();
	if records[first..]
		.iter()
		.any(|record| keys.contains(&(record.app.clone(), record.name.clone())))
	{
		return Err(Error::Invalid(
			"Aidash baseline reversal cannot be faked; apply its backward operations".into(),
		));
	}
	Ok(())
}

/// Reuse Reinhardt's argument grammar and selected settings. Other management
/// commands, including their help and static discovery paths, use the generated driver.
pub async fn manage<P: reinhardt::commands::CapabilityProvider>(
	provider: &P,
) -> Option<reinhardt::commands::CommandResult<()>> {
	use clap::Parser;
	use reinhardt::commands::capabilities::CoreMigrationMetadata;
	use reinhardt::commands::cli::{Cli, Commands};
	use reinhardt::commands::{
		CapabilityContext, CapabilityRequirement, CommandError, SelectedDatabase,
	};
	let cli = Cli::try_parse().ok()?;
	let Commands::Migrate {
		app_label,
		migration_name,
		database,
		fake,
		fake_initial,
		plan,
		migrations_dir,
	} = cli.command
	else {
		return None;
	};
	Some(
		async {
			let override_url = database.or_else(|| std::env::var("DATABASE_URL").ok());
			let mut requirements = vec![CapabilityRequirement::settings::<CoreMigrationMetadata>(
				None,
			)];
			if override_url.is_none() {
				requirements.push(CapabilityRequirement::settings::<SelectedDatabase>(Some(
					"default",
				)));
			}
			let prepared = CapabilityContext::prepare("migrate", &requirements, provider).await?;
			let url = match override_url {
				Some(url) => url,
				None => prepared
					.settings::<SelectedDatabase>(Some("default"))?
					.url(),
			};
			let directory = migrations_dir.unwrap_or_else(|| {
				prepared
					.settings::<CoreMigrationMetadata>(None)
					.expect("prepared migration metadata")
					.base_dir
					.join("migrations")
			});
			let mut context = CommandContext::default();
			context.set_verbosity(cli.verbosity);
			if let Some(app) = app_label {
				context.add_arg(app);
			}
			if let Some(name) = migration_name {
				context.add_arg(name);
			}
			context.set_option("database".into(), url);
			context.set_option(
				"migrations-dir".into(),
				directory.to_string_lossy().into_owned(),
			);
			for (name, enabled) in [
				("fake", fake),
				("fake-initial", fake_initial),
				("plan", plan),
			] {
				if enabled {
					context.set_option(name.into(), "true".into());
				}
			}
			run(&context)
				.await
				.map_err(|error| CommandError::ExecutionError(error.to_string()))
		}
		.await,
	)
}

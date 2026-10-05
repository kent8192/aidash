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

/// All entry points hold the same transaction-scoped lock for the complete run,
/// including creation of the native ledger. Dropping the transaction on errors
/// or cancellation releases it; no session lock can leak into the pool.
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
		refuse_fake_baseline_rollback(context, &applied)?;
	}
	let mut native = context.clone();
	native.set_option(
		"migrations-dir".into(),
		directory.to_string_lossy().into_owned(),
	);
	MigrateCommand
		.execute(&native)
		.await
		.map_err(|error| Error::External(error.to_string()))?;
	schema_lock.commit().await?;
	Ok(())
}

/// Fake reversal would leave physical objects behind after removing baseline
/// records, making the next fresh-schema check and replay unsafe.
fn refuse_fake_baseline_rollback(
	context: &CommandContext,
	applied: &[reinhardt::db::migrations::recorder::MigrationRecord],
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
	#[derive(serde::Deserialize)]
	struct Baseline {
		migrations: Vec<Identity>,
	}
	#[derive(serde::Deserialize)]
	struct Identity {
		app: String,
		name: String,
	}
	let baseline: Baseline = serde_json::from_str(include_str!(concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/migrations/baseline.json"
	)))
	.expect("embedded frozen baseline map is valid");
	let mut keys = HashSet::new();
	for migration in baseline.migrations {
		keys.insert((migration.app.clone(), "0007_model_state".to_owned()));
		keys.insert((migration.app, migration.name));
	}
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

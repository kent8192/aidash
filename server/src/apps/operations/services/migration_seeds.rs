//! Materialize Query builders for Reinhardt's static filesystem migration source.
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use std::path::{Path, PathBuf};

pub struct MigrationSeeds;

fn assets() -> [(&'static str, String); 4] {
	use crate::apps::{
		federation::services::migration_seed as federation,
		marketplace::services::migration_seed as marketplace,
	};
	[
		(
			"federation/sql/forward/0005_seed.sql",
			format!(
				"SET LOCAL search_path = public, pg_catalog;\n\n{};\n",
				federation::forward()
			),
		),
		(
			"federation/sql/backward/0005_seed.sql",
			format!("{};\n", federation::backward()),
		),
		(
			"marketplace/sql/forward/0005_seed.sql",
			format!(
				"SET LOCAL search_path = public, pg_catalog;\n\n{};\n",
				marketplace::forward()
			),
		),
		(
			"marketplace/sql/backward/0005_seed.sql",
			format!("{};\n", marketplace::backward()),
		),
	]
}

fn materialize(root: &Path, write: bool) -> CommandResult<()> {
	for (relative, expected) in assets() {
		let path = root.join(relative);
		if write {
			std::fs::write(&path, expected)?;
		} else if std::fs::read_to_string(&path)? != expected {
			return Err(CommandError::ExecutionError(format!(
				"{} differs from its Reinhardt Query builder; regenerate only an unapplied history with migrationseeds --write",
				path.display()
			)));
		}
	}
	Ok(())
}

#[async_trait]
impl CapabilityCommand for MigrationSeeds {
	fn cli(&self) -> clap::Command {
		clap::Command::new("migrationseeds")
			.about(
				"Check or regenerate Query-backed baseline seed SQL without connecting to services",
			)
			.arg(
				clap::Arg::new("check")
					.long("check")
					.action(clap::ArgAction::SetTrue),
			)
			.arg(
				clap::Arg::new("write")
					.long("write")
					.action(clap::ArgAction::SetTrue)
					.conflicts_with("check")
					.help("Regenerate seed assets for an unapplied history"),
			)
			.arg(
				clap::Arg::new("migration-dir")
					.long("migration-dir")
					.value_parser(clap::value_parser!(PathBuf)),
			)
	}

	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}

	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &CapabilityContext,
	) -> CommandResult<()> {
		let root = matches
			.get_one::<PathBuf>("migration-dir")
			.cloned()
			.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"));
		materialize(&root, matches.get_flag("write"))?;
		println!("Baseline seed assets match their Reinhardt Query builders.");
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use reinhardt::db::migrations::{FilesystemSource, MigrationSource, Operation};
	use rstest::rstest;

	#[rstest]
	#[tokio::test]
	async fn native_seed_operations_load_the_query_generated_assets() {
		// Arrange: the command and native migration source share the canonical files.
		let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
		materialize(&root, false).unwrap();
		let expected = assets();
		// Act and Assert: filesystem parsing retains the exact generated forward/inverse SQL.
		for (index, app) in ["federation", "marketplace"].into_iter().enumerate() {
			let migration = FilesystemSource::new(&root)
				.get_migration(app, "0005_seed")
				.await
				.unwrap();
			assert!(migration.database_only);
			assert_eq!(migration.operations.len(), 1);
			let Operation::RunSQL { sql, reverse_sql } = &migration.operations[0] else {
				panic!("a native filesystem seed must retain its materialized SQL");
			};
			assert_eq!(sql, &expected[index * 2].1);
			assert_eq!(
				reverse_sql.as_deref(),
				Some(expected[index * 2 + 1].1.as_str())
			);
		}
	}
}

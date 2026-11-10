//! Materialize Query builders for Reinhardt's static filesystem migration source.
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use std::path::{Path, PathBuf};

pub struct MigrationSeeds;

fn assets() -> [(&'static str, String); 1] {
	let statement = crate::apps::federation::services::migration_seed::forward()
		.replace('"', "")
		.replace("TRUE", "true");
	[(
		"federation/sql/forward/baseline_seed.sql",
		format!("SET LOCAL search_path = public, pg_catalog;\n\n{statement};\n"),
	)]
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
			.about("Check or regenerate Query-backed migration SQL without connecting to services")
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
		println!("Migration assets match their Reinhardt Query builders.");
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use reinhardt::db::migrations::{FilesystemSource, MigrationSource, Operation};

	#[rstest::rstest]
	#[tokio::test]
	async fn baseline_seed_is_loaded_by_the_filesystem_migration() {
		let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
		materialize(&root, false).unwrap();
		let expected = assets();
		let migration = FilesystemSource::new(&root)
			.get_migration("federation", "0002_tables")
			.await
			.unwrap();
		assert!(migration.database_only);
		assert!(migration.operations.iter().any(
			|operation| matches!(operation, Operation::RunSQL { sql, .. } if sql == &expected[0].1)
		));
	}
}

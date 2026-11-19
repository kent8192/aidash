//! Materialize Query builders for Reinhardt's static filesystem migration source.
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use std::path::{Path, PathBuf};

pub struct MigrationSeeds;

fn assets() -> [(&'static str, String); 31] {
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
		(
			"workspaces/sql/forward/0021_conversation_scratch_space.sql",
			crate::apps::workspaces::services::migration_backfill::forward(),
		),
		(
			"workspaces/sql/backward/0021_conversation_scratch_space.sql",
			crate::apps::workspaces::services::migration_backfill::backward(),
		),
		(
			"workspaces/sql/forward/0024_workspace_scratch_space.sql",
			crate::apps::workspaces::services::workspace_scratch_backfill::forward(),
		),
		(
			"workspaces/sql/backward/0024_workspace_scratch_space.sql",
			crate::apps::workspaces::services::workspace_scratch_backfill::backward(),
		),
		(
			"execution/sql/forward/0090_run_created_at.sql",
			crate::apps::execution::services::migration_backfill::forward(),
		),
		(
			"execution/sql/backward/0090_run_created_at.sql",
			crate::apps::execution::services::migration_backfill::backward(),
		),
		(
			"identity/sql/forward/0067_scratch_member_policy.sql",
			format!(
				"SET LOCAL aidash.scope = 'deployment';\n{};\n",
				crate::apps::identity::services::scratch_policy_backfill::forward()
			),
		),
		(
			"identity/sql/backward/0067_scratch_member_policy.sql",
			crate::apps::identity::services::scratch_policy_backfill::backward(),
		),
		(
			"identity/sql/forward/0080_schedule_member_policy.sql",
			format!(
				"SET LOCAL aidash.scope = 'deployment';\n{};\n",
				crate::apps::identity::services::duty_policy_backfill::forward()
			),
		),
		(
			"identity/sql/backward/0080_schedule_member_policy.sql",
			crate::apps::identity::services::duty_policy_backfill::backward(),
		),
		(
			"identity/sql/forward/0082_remove_package_index_approvals.sql",
			format!(
				"SET LOCAL aidash.scope = 'deployment';\n{};\n",
				crate::apps::identity::services::package_index_approvals::forward()
			),
		),
		(
			"identity/sql/backward/0082_remove_package_index_approvals.sql",
			crate::apps::identity::services::package_index_approvals::backward(),
		),
		(
			"identity/sql/forward/0083_data_protection_switches.sql",
			crate::apps::identity::services::data_protection_backfill::forward(),
		),
		(
			"identity/sql/backward/0083_data_protection_switches.sql",
			crate::apps::identity::services::data_protection_backfill::backward(),
		),
		(
			"identity/sql/forward/0085_remove_research_approvals.sql",
			crate::apps::identity::services::research_approval_cleanup::forward(),
		),
		(
			"execution/sql/forward/0141_remove_research_records.sql",
			crate::apps::execution::services::research_record_cleanup::forward(),
		),
		(
			"execution/sql/backward/0141_remove_research_records.sql",
			crate::apps::execution::services::research_record_cleanup::backward(),
		),
		(
			"identity/sql/forward/0098_duty_member_policy.sql",
			crate::apps::identity::services::duty_actions_backfill::forward(),
		),
		(
			"identity/sql/backward/0098_duty_member_policy.sql",
			crate::apps::identity::services::duty_actions_backfill::backward(),
		),
		(
			"identity/sql/forward/0110_reply_thread_modes.sql",
			crate::apps::workspaces::services::thread_backfill::reply_thread_modes(),
		),
		(
			"identity/sql/forward/0111_thread_member_policy.sql",
			crate::apps::identity::services::thread_policy_backfill::forward(),
		),
		(
			"identity/sql/backward/0111_thread_member_policy.sql",
			crate::apps::identity::services::thread_policy_backfill::backward(),
		),
		(
			"execution/sql/forward/0148_duplicate_areas.sql",
			crate::apps::workspaces::services::thread_backfill::duplicate_areas(),
		),
		(
			"execution/sql/forward/0150_execution_events.sql",
			crate::apps::execution::services::execution_event_backfill::forward(),
		),
		(
			"execution/sql/backward/0150_execution_events.sql",
			crate::apps::execution::services::execution_event_backfill::backward(),
		),
		(
			"workspaces/sql/forward/0033_duty_halts.sql",
			crate::apps::workspaces::services::duty_halt_backfill::forward(),
		),
		(
			"workspaces/sql/backward/0033_duty_halts.sql",
			crate::apps::workspaces::services::duty_halt_backfill::backward(),
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
		let migration = FilesystemSource::new(&root)
			.get_migration("workspaces", "0021_conversation_scratch_space")
			.await
			.unwrap();
		let Operation::RunSQL { sql, reverse_sql } = &migration.operations[1] else {
			panic!("missing conversation backfill");
		};
		assert_eq!(sql, &expected[4].1);
		assert_eq!(reverse_sql.as_deref(), Some(expected[5].1.as_str()));
		let migration = FilesystemSource::new(&root)
			.get_migration("workspaces", "0024_workspace_scratch_space")
			.await
			.unwrap();
		let Operation::RunSQL { sql, reverse_sql } = &migration.operations[2] else {
			panic!("missing Workspace backfill");
		};
		assert_eq!(sql, &expected[6].1);
		assert_eq!(reverse_sql.as_deref(), Some(expected[7].1.as_str()));
		let migration = FilesystemSource::new(&root)
			.get_migration("identity", "0067_scratch_member_policy")
			.await
			.unwrap();
		assert!(migration.database_only);
		assert!(matches!(migration.operations.as_slice(),
			[Operation::RunSQL { sql, reverse_sql: Some(reverse) }] if sql == &expected[10].1 && reverse == &expected[11].1));
		let migration = FilesystemSource::new(&root)
			.get_migration("identity", "0080_schedule_member_policy")
			.await
			.unwrap();
		assert!(migration.database_only);
		assert!(matches!(migration.operations.as_slice(),
			[Operation::RunSQL { sql, reverse_sql: Some(reverse) }] if sql == &expected[12].1 && reverse == &expected[13].1));
		// The data rewrites of #962, #963 and #987 run beside typed and
		// handwritten DDL operations of the same migrations.
		for (app, name, forward, backward) in [
			("identity", "0083_data_protection_switches", 16, Some(17)),
			("identity", "0085_remove_research_approvals", 18, None),
			("execution", "0141_remove_research", 19, Some(20)),
			("identity", "0098_duty_member_policy", 21, Some(22)),
			("workspaces", "0033_duty_checks", 23, Some(24)),
		] {
			let migration = FilesystemSource::new(&root)
				.get_migration(app, name)
				.await
				.unwrap();
			assert!(
				migration.operations.iter().any(|operation| matches!(operation,
					Operation::RunSQL { sql, reverse_sql }
					if sql == &expected[forward].1
						&& backward.is_none_or(|backward| reverse_sql.as_deref() == Some(expected[backward].1.as_str()))
				)),
				"{app}/{name} runs its generated rewrite"
			);
		}
		let migration = FilesystemSource::new(&root)
			.get_migration("execution", "0090_run_created_at")
			.await
			.unwrap();
		assert!(
			migration
				.operations
				.iter()
				.any(|operation| matches!(operation,
					Operation::RunSQL { sql, reverse_sql }
					if sql == &expected[8].1 && reverse_sql.as_deref() == Some(expected[9].1.as_str())
				))
		);
	}
}

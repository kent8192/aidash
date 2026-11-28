//! Materialize Query builders for Reinhardt's static filesystem migration source.
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandError, CommandResult,
};
use std::path::{Path, PathBuf};

pub struct MigrationSeeds;

fn assets() -> [(&'static str, String); 41] {
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
			"federation/sql/forward/0031_transaction_parts.sql",
			crate::apps::federation::services::transaction_journal_reset::forward(),
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
			"identity/sql/backward/0113_budget_notice_delivery.sql",
			crate::apps::identity::repositories::budgets::rollback_notices(),
		),
		(
			"identity/sql/forward/0114_policy_revision_backfill.sql",
			crate::apps::identity::services::policy_revision_backfill::forward(),
		),
		(
			"execution/sql/forward/0148_duplicate_areas.sql",
			crate::apps::workspaces::services::thread_backfill::duplicate_areas(),
		),
		(
			"workspaces/sql/forward/0033_duty_halts.sql",
			crate::apps::workspaces::services::duty_halt_backfill::forward(),
		),
		(
			"workspaces/sql/backward/0033_duty_halts.sql",
			crate::apps::workspaces::services::duty_halt_backfill::backward(),
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
			"execution/sql/forward/0153_area_queue_sleep_backfill.sql",
			crate::apps::execution::services::area_queue_sleep_backfill::forward(),
		),
		(
			"workspaces/sql/forward/0067_task_duty_start.sql",
			crate::apps::workspaces::services::duty_start_backfill::forward(),
		),
		(
			"workspaces/sql/backward/0067_task_duty_start.sql",
			crate::apps::workspaces::services::duty_start_backfill::backward(),
		),
		(
			"registry/sql/forward/0083_keyless_openai_compatible.sql",
			crate::apps::registry::services::keyless_provider_backfill::forward(),
		),
		(
			"registry/sql/backward/0083_keyless_openai_compatible.sql",
			crate::apps::registry::services::keyless_provider_backfill::backward(),
		),
		(
			"knowledge/sql/forward/0060_keyless_openai_compatible.sql",
			crate::apps::knowledge::services::keyless_embedding_backfill::forward(),
		),
		(
			"knowledge/sql/backward/0060_keyless_openai_compatible.sql",
			crate::apps::knowledge::services::keyless_embedding_backfill::backward(),
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
	use std::collections::BTreeMap;

	#[rstest]
	#[tokio::test]
	async fn native_seed_operations_load_the_query_generated_assets() {
		// Arrange: the command and native migration source share the canonical files.
		let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
		materialize(&root, false).unwrap();
		let expected: BTreeMap<_, _> = assets().into_iter().collect();
		// Act and Assert: filesystem parsing retains the exact generated forward/inverse SQL.
		for app in ["federation", "marketplace"] {
			let migration = FilesystemSource::new(&root)
				.get_migration(app, "0005_seed")
				.await
				.unwrap();
			assert!(migration.database_only);
			assert_eq!(migration.operations.len(), 1);
			let Operation::RunSQL { sql, reverse_sql } = &migration.operations[0] else {
				panic!("a native filesystem seed must retain its materialized SQL");
			};
			assert_eq!(
				sql,
				&expected[format!("{app}/sql/forward/0005_seed.sql").as_str()]
			);
			assert_eq!(
				reverse_sql.as_deref(),
				Some(expected[format!("{app}/sql/backward/0005_seed.sql").as_str()].as_str())
			);
		}
		let migration = FilesystemSource::new(&root)
			.get_migration("workspaces", "0021_conversation_scratch_space")
			.await
			.unwrap();
		let Operation::RunSQL { sql, reverse_sql } = &migration.operations[1] else {
			panic!("missing conversation backfill");
		};
		assert_eq!(
			sql,
			&expected["workspaces/sql/forward/0021_conversation_scratch_space.sql"]
		);
		assert_eq!(
			reverse_sql.as_deref(),
			Some(expected["workspaces/sql/backward/0021_conversation_scratch_space.sql"].as_str())
		);
		let migration = FilesystemSource::new(&root)
			.get_migration("workspaces", "0024_workspace_scratch_space")
			.await
			.unwrap();
		let Operation::RunSQL { sql, reverse_sql } = &migration.operations[2] else {
			panic!("missing Workspace backfill");
		};
		assert_eq!(
			sql,
			&expected["workspaces/sql/forward/0024_workspace_scratch_space.sql"]
		);
		assert_eq!(
			reverse_sql.as_deref(),
			Some(expected["workspaces/sql/backward/0024_workspace_scratch_space.sql"].as_str())
		);
		let migration = FilesystemSource::new(&root)
			.get_migration("identity", "0067_scratch_member_policy")
			.await
			.unwrap();
		assert!(migration.database_only);
		assert!(matches!(migration.operations.as_slice(),
			[Operation::RunSQL { sql, reverse_sql: Some(reverse) }]
				if sql == &expected["identity/sql/forward/0067_scratch_member_policy.sql"]
					&& reverse == &expected["identity/sql/backward/0067_scratch_member_policy.sql"]));
		let migration = FilesystemSource::new(&root)
			.get_migration("identity", "0080_schedule_member_policy")
			.await
			.unwrap();
		assert!(migration.database_only);
		assert!(matches!(migration.operations.as_slice(),
			[Operation::RunSQL { sql, reverse_sql: Some(reverse) }]
				if sql == &expected["identity/sql/forward/0080_schedule_member_policy.sql"]
					&& reverse == &expected["identity/sql/backward/0080_schedule_member_policy.sql"]));
		// The data rewrites of #962, #963, #987 and #1008 run beside typed and
		// handwritten DDL operations of the same migrations.
		for (app, name, asset, check_reverse) in [
			(
				"identity",
				"0083_data_protection_switches",
				"0083_data_protection_switches",
				true,
			),
			(
				"identity",
				"0085_remove_research_approvals",
				"0085_remove_research_approvals",
				false,
			),
			// The journal reset of #1107 before the coordinator seed column.
			(
				"federation",
				"0031_transaction_parts",
				"0031_transaction_parts",
				false,
			),
			(
				"execution",
				"0141_remove_research",
				"0141_remove_research_records",
				true,
			),
			(
				"identity",
				"0098_duty_member_policy",
				"0098_duty_member_policy",
				true,
			),
			// The Thread rewrites of #1008.
			("identity", "0110_threads", "0110_reply_thread_modes", false),
			(
				"identity",
				"0111_thread_member_policy",
				"0111_thread_member_policy",
				true,
			),
			(
				"identity",
				"0114_policy_revision_backfill",
				"0114_policy_revision_backfill",
				false,
			),
			("execution", "0148_threads", "0148_duplicate_areas", false),
			("workspaces", "0033_duty_checks", "0033_duty_halts", true),
			(
				"execution",
				"0150_task_executions",
				"0150_execution_events",
				true,
			),
		] {
			let migration = FilesystemSource::new(&root)
				.get_migration(app, name)
				.await
				.unwrap();
			let forward = format!("{app}/sql/forward/{asset}.sql");
			let backward = format!("{app}/sql/backward/{asset}.sql");
			assert!(
				migration
					.operations
					.iter()
					.any(|operation| matches!(operation,
						Operation::RunSQL { sql, reverse_sql }
						if sql == &expected[forward.as_str()]
							&& (!check_reverse || reverse_sql.as_deref() == Some(expected[backward.as_str()].as_str()))
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
					if sql == &expected["execution/sql/forward/0090_run_created_at.sql"]
						&& reverse_sql.as_deref()
							== Some(expected["execution/sql/backward/0090_run_created_at.sql"].as_str())
				))
		);
	}
}

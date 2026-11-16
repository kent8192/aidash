// reinhardt-migration-source: 1
// Agent Bindings accept a pasted secret per remote server of an Agent Plugin
// (#954, ADR 0073 amended 2026-11-16, ADR 0100): `agent_plugin_settings`
// gains `secret` beside `tools`, `exposure` and `read_only`. The Agent
// contract is a PostgreSQL validation function, and Reinhardt has no typed
// operation for replacing a function body, so this DDL-only migration uses SQL
// assets. Like 0061 and 0065 it validates and strips the new key in a wrapper,
// then delegates to the existing body.
//
// Numbered 0071 after develop's leaf 0068: open PR #952 already takes registry
// 0069 and 0070. Whichever of the two lands second renumbers or adds a merge
// migration.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0071_agent_plugin_server_secrets", "registry")
		.add_dependency("registry", "0068_reject_new_compaction_deciders")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0071_agent_plugin_server_secrets.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0071_agent_plugin_server_secrets.sql").into(),
			),
		})
}

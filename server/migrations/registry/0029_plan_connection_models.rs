// reinhardt-migration-source: 1
// Plan Connection models (#159, ADR 0007): admit the `chatgpt` provider, served
// only by the optional `plan_connection` key, beside OpenRouter. Edit only the
// CURRENT registry_model_config read from the catalog, so the allowlist
// additions of 0016-0020 survive, and add registry_plan_connection_config:
// `chatgpt` models name `plan_connection = "chatgpt"` and OpenAI's endpoint,
// never a Provider Credential, and no other model names a Plan Connection.
// PostgreSQL JSONB CHECK expressions and catalog-driven constraint edits have
// no typed Reinhardt operation; the filesystem migration loader supports
// external assets through RunSQL only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0029_plan_connection_models", "registry")
		.database_only(true)
		.add_dependency("registry", "0028_tool_transport_secrets")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0029_plan_connection_models.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0029_plan_connection_models.sql").into()),
		})
		.atomic(true)
}

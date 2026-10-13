// reinhardt-migration-source: 1
// ADR 0006: model and embedding configurations obtain keys only from Tenant
// Provider Credentials. Reject the removed `credential_env` reference so the
// database agrees with the application contract. Not deployed yet, so there is
// no data rewrite; a database holding such entries must re-register them.
// Installation overrides are covered too: the SQL assets refuse the upgrade
// while a model or embedding override still sets `credential_env` and make the
// installation guard's model section reject the key. Data-dependent refusals
// and catalog-driven function edits have no typed Reinhardt operation; the
// filesystem migration loader supports external assets through RunSQL only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0023_model_provider_keys", "registry")
		.database_only(true)
		.add_dependency("registry", "0022_context_policy")
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_model_provider_keys".into(),
				expression: "kind NOT IN ('model', 'embedding') OR NOT ((metadata -> 'config') ? 'credential_env')".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0023_model_provider_keys.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0023_model_provider_keys.sql").into()),
		})
		.atomic(true)
}

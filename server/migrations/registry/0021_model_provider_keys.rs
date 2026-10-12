// reinhardt-migration-source: 1
// ADR 0006: model and embedding configurations obtain keys only from Tenant
// Provider Credentials. Reject the removed `credential_env` reference so the
// database agrees with the application contract. Not deployed yet, so there is
// no data rewrite; a database holding such entries must re-register them.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0021_model_provider_keys", "registry")
		.database_only(true)
		.add_dependency("registry", "0020_model_streaming_config")
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_model_provider_keys".into(),
				expression: "kind NOT IN ('model', 'embedding') OR NOT ((metadata -> 'config') ? 'credential_env')".into(),
			},
		})
}

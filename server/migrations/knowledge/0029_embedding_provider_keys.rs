// reinhardt-migration-source: 1
// ADR 0006: semantic index embeddings obtain keys only from Tenant Provider
// Credentials. Reject the removed `credential_env` reference; the vector store
// reference is not a model-provider key and is unchanged.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0029_embedding_provider_keys", "knowledge")
		.database_only(true)
		.add_dependency("knowledge", "0028_provider_credentials")
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_indexes".into(),
			constraint: Constraint::Check {
				name: "semantic_embedding_provider_keys".into(),
				expression: "NOT ((spec -> 'embedding') ? 'credential_env')".into(),
			},
		})
}

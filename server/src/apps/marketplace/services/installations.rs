use super::*;
use crate::{
	Result,
	authorization::{access::Access, policy::Resource},
	store::Store,
};
use serde_json::json;

pub(crate) fn resource(access: &Access, install: &Installation, revision: Option<i64>) -> Resource {
	access.resource("installation",&install.id,json!({"installing_tenant":install.tenant,"package_key":install.package_key,"installation_revision":revision}))
}

pub(crate) async fn propagate_provenance(
	tx: &mut crate::database::native::Transaction,
	source: &EntityRef,
	target: &Entry,
	tenant: &str,
) -> Result<()> {
	aidash_application::marketplace::installations::propagate_provenance(
		&mut crate::bootstrap::marketplace_provenance_scope(tx),
		source,
		target,
		tenant,
	)
	.await
	.map_err(Into::into)
}

/// Only new discovery/admission requires the selected active pointer.
pub(crate) async fn active(access: &mut Access, entry: &Entry) -> Result<bool> {
	aidash_application::marketplace::installations::active(
		&mut crate::bootstrap::marketplace_distribution_scope(access),
		entry,
	)
	.await
	.map_err(Into::into)
}

pub(super) async fn view(
	access: &mut Access,
	id: &str,
	rev: Option<i64>,
	node: &str,
) -> Result<InstallationRevision> {
	aidash_application::marketplace::installations::view(
		&mut crate::bootstrap::marketplace_distribution_scope(access),
		id,
		rev,
		node,
	)
	.await
	.map_err(Into::into)
}
fn install_command(
	input: &Install,
) -> aidash_application::marketplace::installations::InstallCommand {
	aidash_application::marketplace::installations::InstallCommand {
		digest: input.digest.clone(),
		config: input.config.clone(),
		bindings: input.bindings.clone(),
		idempotency_key: input.idempotency_key,
	}
}
fn configure_command(
	input: &Configure,
) -> aidash_application::marketplace::installations::ConfigureCommand {
	aidash_application::marketplace::installations::ConfigureCommand {
		expected_revision: input.expected_revision,
		config: input.config.clone(),
		bindings: input.bindings.clone(),
		idempotency_key: input.idempotency_key,
	}
}
pub(super) async fn install(
	store: &Store,
	access: &mut Access,
	package: &str,
	input: &Install,
) -> Result<InstallationRevision> {
	aidash_application::marketplace::installations::install(
		&mut crate::bootstrap::marketplace_publication_scope(store, access),
		&crate::bootstrap::registry_validation_for(store),
		package,
		&install_command(input),
		&store.node_id,
	)
	.await
	.map_err(Into::into)
}
pub(super) async fn configure(
	store: &Store,
	access: &mut Access,
	id: &str,
	input: &Configure,
) -> Result<InstallationRevision> {
	aidash_application::marketplace::installations::configure(
		&mut crate::bootstrap::marketplace_publication_scope(store, access),
		&crate::bootstrap::registry_validation_for(store),
		id,
		&configure_command(input),
		&store.node_id,
	)
	.await
	.map_err(Into::into)
}

pub(crate) use super::super::repositories::installations::{provenance, revision};

#[cfg(test)]
mod conversion_tests {
	use super::*;
	use aidash_domain::marketplace::definitions::key;
	use reinhardt::core::validators::Validate as _;
	use rstest::rstest;
	#[rstest]
	fn installation_conversion_preserves_replay_bytes() {
		let input = Install {
			digest: "sha256:fixture".into(),
			config: json!({"instructions":"Frozen"}),
			bindings: vec![DependencyBinding {
				source: EntityRef {
					id: "source".into(),
					version: "1.0.0".into(),
				},
				target: EntityRef {
					id: "target".into(),
					version: "2.0.0".into(),
				},
			}],
			idempotency_key: Uuid::nil(),
		};
		input.validate().unwrap();
		let converted = install_command(&input);
		assert_eq!(
			serde_json::to_vec(&input).unwrap(),
			serde_json::to_vec(&converted).unwrap()
		);
		assert_eq!(key(&("package", &input)), key(&("package", &converted)));
	}
	#[rstest]
	fn configuration_conversion_preserves_replay_bytes() {
		let input = Configure {
			expected_revision: 7,
			config: json!({"instructions":"Changed"}),
			bindings: vec![DependencyBinding {
				source: EntityRef {
					id: "source".into(),
					version: "1.0.0".into(),
				},
				target: EntityRef {
					id: "target".into(),
					version: "2.0.0".into(),
				},
			}],
			idempotency_key: Uuid::nil(),
		};
		input.validate().unwrap();
		let converted = configure_command(&input);
		assert_eq!(
			serde_json::to_vec(&input).unwrap(),
			serde_json::to_vec(&converted).unwrap()
		);
		assert_eq!(
			key(&("installation", &input)),
			key(&("installation", &converted))
		);
	}
}

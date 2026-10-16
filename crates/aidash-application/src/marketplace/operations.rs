//! Deployment Administrator-only activation and Tenant installation administration.
use super::definitions;
use crate::{
	Error, Result, ports::marketplace::DeploymentAdministratorScope, registry::DefinitionValidation,
};
use aidash_domain::{
	marketplace::definitions::{content, key, manifest, prospective_installation, reference},
	marketplace::installations::activation,
	marketplace::{Dependency, Installation, InstallationRevision, Version},
	registry::EntityRef,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

pub struct ActivationCommand {
	pub tenant: String,
	pub revision: i64,
	pub expected_activation_revision: i64,
	pub expected_catalog_revision: i64,
	pub enabled: bool,
}
pub struct AdministrationQuery {
	pub tenant: String,
	pub offset: usize,
	pub limit: usize,
}
pub struct AdministrationPage {
	pub entries: Vec<InstallationRevision>,
	pub next_offset: Option<usize>,
}

fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}
pub async fn activate(
	scope: &mut dyn DeploymentAdministratorScope,
	id: &str,
	input: &ActivationCommand,
) -> Result<Installation> {
	require_deployment_administrator(scope.principal())?;
	scope.load_tenant(&input.tenant).await?;
	scope.distribution_lock(true).await?;
	scope.enable_writer().await?;
	let install = scope.installation(id).await?.ok_or(Error::Forbidden)?;
	if install.tenant != input.tenant {
		return Err(Error::Forbidden);
	}
	if install.activation_revision != input.expected_activation_revision {
		return Err(conflict());
	}
	let revision = scope.revision(id, input.revision).await?;
	if input.enabled {
		for dependency in &revision.dependencies {
			if !scope.approved(&input.tenant, dependency).await? {
				return Err(Error::Forbidden);
			}
		}
	}
	scope
		.set_approval(
			&input.tenant,
			&reference(&revision.entry),
			input.expected_catalog_revision,
			input.enabled,
		)
		.await?;
	let install = activation(install, input.revision, input.enabled);
	scope.save_installation(&install).await?;
	scope.event("marketplace.activation_changed", json!({"installation":id,"tenant":input.tenant,"revision":input.revision,"enabled":input.enabled,"actor":"deployment_administrator"})).await?;
	Ok(install)
}
pub async fn administration(
	scope: &mut dyn DeploymentAdministratorScope,
	input: &AdministrationQuery,
) -> Result<AdministrationPage> {
	require_deployment_administrator(scope.principal())?;
	aidash_domain::policy::identifier(&input.tenant)?;
	scope.distribution_lock(false).await?;
	let mut rows = scope
		.revision_documents(&input.tenant, input.offset, (input.limit + 1) as u64)
		.await?;
	let more = rows.len() > input.limit;
	rows.truncate(input.limit);
	let mut entries = vec![];
	for (installation, revision) in rows {
		let installation: Installation = serde_json::from_value(installation)?;
		let revision: aidash_domain::marketplace::Revision = serde_json::from_value(revision)?;
		let approved = scope
			.approved(&input.tenant, &reference(&revision.entry))
			.await?;
		entries.push(InstallationRevision {
			installation,
			revision: revision.revision,
			entry: revision.entry,
			digest: revision.digest,
			config: revision.config,
			dependencies: revision.dependencies,
			bindings: revision.bindings,
			approved,
			actions: vec![],
		});
	}
	Ok(AdministrationPage {
		entries,
		next_offset: more.then_some(input.offset + input.limit),
	})
}
#[cfg(test)]
mod tests;

pub use crate::authorization::require_deployment_administrator;

pub mod approval_set;
pub mod host_packages;

//! Immutable installation revisions, current disclosure and idempotent configuration.
use super::{definitions, distribution};
use crate::{
	Error, Result,
	ports::marketplace::{DefinitionScope, InstallationRead, InstallationScope, StagingScope},
	registry::DefinitionValidation,
};
use aidash_domain::{
	marketplace::definitions::{
		installation_id as id, key, manifest, prospective_installation as prospective, reference,
	},
	marketplace::installations::{same, stage_revision},
	marketplace::*,
	registry::{EntityRef, Entry},
};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

/// Serialize in the same order as the existing transport contract for replay keys.
#[derive(Clone, Serialize)]
pub struct InstallCommand {
	pub digest: String,
	pub config: Value,
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}
#[derive(Clone, Serialize)]
pub struct ConfigureCommand {
	pub expected_revision: i64,
	pub config: Value,
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}
fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}

pub async fn owned(scope: &mut dyn InstallationRead, id: &str) -> Result<Installation> {
	let install: Installation = scope.installation(id).await?.ok_or(Error::Forbidden)?;
	if install.tenant != scope.tenant() {
		return Err(Error::Forbidden);
	}
	scope
		.require(
			&scope.installation_resource(&install, None),
			"installation.read",
		)
		.await?;
	Ok(install)
}
pub async fn view(
	scope: &mut dyn InstallationRead,
	id: &str,
	rev: Option<i64>,
	node: &str,
) -> Result<InstallationRevision> {
	scope.operation(
		"installation.read",
		json!({"installation":id,"revision":rev}),
		None,
	);
	let install = owned(scope, id).await?;
	let revision = scope
		.revision(id, rev.unwrap_or(install.latest_revision))
		.await?;
	scope
		.require(
			&scope.installation_resource(&install, Some(revision.revision)),
			"installation.read",
		)
		.await?;
	definitions::local_graph(
		scope,
		revision
			.dependencies
			.iter()
			.cloned()
			.map(|r| (r, String::new()))
			.collect(),
		node,
	)
	.await?;
	definitions::private_context(scope, &revision.entry).await?;
	scope.audit_installation(json!({"id":id,"revision":revision.revision,"digest":revision.digest,"entry":reference(&revision.entry)}));
	let approved = scope
		.approved(&install.tenant, &reference(&revision.entry))
		.await?;
	let actions = if scope
		.decide(
			&scope.installation_resource(&install, Some(revision.revision)),
			"installation.configure",
		)
		.await?
	{
		vec!["configure".into()]
	} else {
		vec![]
	};
	Ok(InstallationRevision {
		installation: install,
		revision: revision.revision,
		entry: revision.entry,
		digest: revision.digest,
		config: revision.config,
		dependencies: revision.dependencies,
		bindings: revision.bindings,
		approved,
		actions,
	})
}
pub async fn install(
	scope: &mut dyn InstallationScope,
	validation: &DefinitionValidation,
	package: &str,
	input: &InstallCommand,
	node: &str,
) -> Result<InstallationRevision> {
	scope.operation(
		"marketplace.install",
		json!({"package":package,"digest":input.digest,"installation":id(scope.tenant(),package)}),
		Some(input.idempotency_key),
	);
	let source = distribution::load(scope, package, "marketplace.read").await?;
	distribution::readable(scope, &source, node).await?;
	scope
		.require(&scope.package_resource(&source), "marketplace.install")
		.await?;
	let install = prospective(scope.tenant(), package);
	for action in ["installation.create", "installation.read"] {
		scope
			.require(&scope.installation_resource(&install, None), action)
			.await?;
	}
	if source.digest != input.digest {
		return Err(conflict());
	}
	let fingerprint = key(&(package, input));
	if let Some(saved) = replay(scope, "install", input.idempotency_key, &fingerprint, node).await?
	{
		return view(scope, &install.id, saved["revision"].as_i64(), node).await;
	}
	let resolved = definitions::resolve(
		scope,
		validation,
		&source,
		&input.config,
		&input.bindings,
		node,
	)
	.await?;
	if let Some(existing) = scope.installation(&install.id).await? {
		let current = scope
			.revision(&install.id, existing.latest_revision)
			.await?;
		if !same(&current, &resolved.0, &input.config, &resolved.2) {
			return Err(conflict());
		}
		remember(
			scope,
			"install",
			input.idempotency_key,
			fingerprint,
			json!({"installation":install.id,"revision":current.revision}),
		)
		.await?;
		return view(scope, &install.id, Some(current.revision), node).await;
	}
	let revision = stage(
		scope,
		validation,
		install,
		&source,
		input.config.clone(),
		resolved,
	)
	.await?;
	let installation = id(scope.tenant(), package);
	remember(
		scope,
		"install",
		input.idempotency_key,
		fingerprint,
		json!({"installation":installation,"revision":revision}),
	)
	.await?;
	view(scope, &installation, Some(revision), node).await
}
pub async fn configure(
	scope: &mut dyn InstallationScope,
	validation: &DefinitionValidation,
	id: &str,
	input: &ConfigureCommand,
	node: &str,
) -> Result<InstallationRevision> {
	if input.expected_revision < 1 || input.expected_revision == i64::MAX {
		return Err(Error::Invalid("invalid installation revision".into()));
	}
	scope.operation(
		"installation.configure",
		json!({"installation":id,"expected_revision":input.expected_revision}),
		Some(input.idempotency_key),
	);
	let install = owned(scope, id).await?;
	scope
		.require(
			&scope.installation_resource(&install, Some(install.latest_revision)),
			"installation.configure",
		)
		.await?;
	let current = scope.revision(id, install.latest_revision).await?;
	let fingerprint = key(&(id, input));
	if let Some(saved) = replay(
		scope,
		"configure",
		input.idempotency_key,
		&fingerprint,
		node,
	)
	.await?
	{
		return view(scope, id, saved["revision"].as_i64(), node).await;
	}
	let resolved = definitions::resolve(
		scope,
		validation,
		&current.source,
		&input.config,
		&input.bindings,
		node,
	)
	.await?;
	// Unchanged retries do not manufacture a new pending revision, including
	// retries with an old optimistic revision after the identical change won.
	let next = if same(&current, &resolved.0, &input.config, &resolved.2) {
		current.revision
	} else {
		if input.expected_revision != install.latest_revision {
			return Err(conflict());
		}
		stage(
			scope,
			validation,
			install,
			&current.source,
			input.config.clone(),
			resolved,
		)
		.await?
	};
	remember(
		scope,
		"configure",
		input.idempotency_key,
		fingerprint,
		json!({"installation":id,"revision":next}),
	)
	.await?;
	view(scope, id, Some(next), node).await
}

async fn replay(
	scope: &mut dyn InstallationScope,
	operation: &str,
	request: Uuid,
	fingerprint: &str,
	node: &str,
) -> Result<Option<Value>> {
	let Some(saved) = scope.installation_replay(operation, request).await? else {
		return Ok(None);
	};
	if saved.fingerprint != fingerprint {
		// A revoked installation hides its old idempotency result and conflict.
		view(
			scope,
			saved.result["installation"]
				.as_str()
				.ok_or(Error::Forbidden)?,
			saved.result["revision"].as_i64(),
			node,
		)
		.await?;
		return Err(Error::Conflict(
			"idempotency key has different input".into(),
		));
	}
	Ok(Some(saved.result))
}
async fn remember(
	scope: &mut dyn InstallationScope,
	operation: &str,
	request: Uuid,
	fingerprint: String,
	result: Value,
) -> Result<()> {
	scope
		.remember_installation(operation, request, fingerprint, result)
		.await
}

/// Preserve protected write, private-document copy, provenance and outbox order.
pub async fn stage(
	scope: &mut dyn StagingScope,
	validation: &DefinitionValidation,
	installation: Installation,
	source: &Version,
	config: Value,
	resolved: (Entry, Vec<EntityRef>, Vec<DependencyBinding>),
) -> Result<i64> {
	let staged = stage_revision(
		installation,
		source,
		config,
		resolved,
		scope.allocate_entry_id(),
	);
	validation.validate_in(&staged.revision.entry, false)?;
	scope
		.persist_revision(&staged.installation, &staged.revision)
		.await?;
	let entry = &staged.revision.entry;
	if entry.kind == "agent"
		&& entry
			.config
			.get("knowledge_digest")
			.is_some_and(|d| !d.is_null())
	{
		let original = manifest(source)?.entity;
		let documents = scope.documents(&original).await?.ok_or(Error::Forbidden)?;
		if entry.config["knowledge_digest"].as_str()
			!= Some(aidash_domain::registry::knowledge::digest(&documents).as_str())
		{
			return Err(Error::Forbidden);
		}
		scope.insert_documents(entry, documents).await?;
	}
	scope.save_provenance(entry, &staged.provenance).await?;
	let mut copies = scope
		.copy_provenance(entry, &staged.installation.tenant)
		.await?;
	copies.extend(staged.provenance);
	scope
		.save_copy_provenance(entry, &staged.installation.tenant, &copies)
		.await?;
	scope.installed_event(json!({"installation":staged.installation.id,"tenant":staged.installation.tenant,"revision":staged.revision.revision,"digest":staged.revision.digest,"actor":scope.actor()})).await?;
	Ok(staged.revision.revision)
}

#[cfg(test)]
pub(crate) mod tests;

/// Discovery/admission requires the selected pointer; retained runtime references
/// continue to use their immutable revision and current individual catalog grants.
pub async fn active(scope: &mut dyn InstallationRead, entry: &Entry) -> Result<bool> {
	let Some(projection) = &entry.installation else {
		return Ok(true);
	};
	if projection.tenant != scope.tenant() || projection.contract != 1 {
		return Ok(false);
	}
	match scope.installation_gate().await {
		Ok(()) => {}
		Err(Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	}
	let Some(installation) = scope
		.selected_installation(&projection.installation)
		.await?
	else {
		return Ok(false);
	};
	Ok(installation.tenant == projection.tenant
		&& installation.active_revision == Some(projection.revision))
}
/// Recheck every immutable dependency at each worker boundary.
pub async fn check_pinned(scope: &mut dyn DefinitionScope, entry: &Entry) -> Result<()> {
	if let Some(projection) = &entry.installation {
		let revision = scope
			.revision(&projection.installation, projection.revision)
			.await?;
		for dependency in revision.dependencies {
			scope.catalog(&dependency, "registry.read").await?;
		}
	}
	Ok(())
}

pub async fn propagate_provenance(
	scope: &mut dyn crate::ports::marketplace::ProvenanceScope,
	source: &EntityRef,
	target: &Entry,
	tenant: &str,
) -> Result<()> {
	let source = scope.raw_definition(source).await?;
	let edges = scope.provenance(&source, tenant).await?;
	if !edges.is_empty() {
		scope.save_provenance(target, &edges).await?;
	}
	Ok(())
}

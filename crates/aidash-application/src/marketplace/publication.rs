//! Immutable publication and distribution changes under one current authority scope.
use super::{
	definitions,
	distribution::{consent, load, readable},
};
use crate::{
	Error, Result,
	ports::marketplace::{DistributionWriter, PublicationScope},
	registry::DefinitionValidation,
};
use aidash_domain::{
	marketplace::definitions::{key, manifest},
	marketplace::publication::validate_metadata,
	marketplace::*,
	policy::identifier,
	registry::{EntityRef, Package},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Field order preserves the existing serialized idempotency fingerprint.
#[derive(Clone, Serialize)]
pub struct PublishCommand {
	pub source: EntityRef,
	pub package_id: String,
	pub author: String,
	pub permissions: Vec<String>,
	pub dependencies: Vec<EntityRef>,
	pub idempotency_key: Uuid,
}

pub struct AudienceChange {
	pub expected_revision: i64,
	pub tenants: BTreeSet<String>,
}
fn conflict() -> Error {
	Error::Conflict("revision or immutable content changed".into())
}

pub async fn prepare(
	scope: &mut dyn PublicationScope,
	validation: &DefinitionValidation,
	input: &PublishCommand,
	node: &str,
) -> Result<Version> {
	validate_metadata(
		&input.package_id,
		&input.author,
		input.dependencies.len(),
		input.permissions.len(),
	)?;
	let tenant = scope.tenant().to_owned();
	let stored = scope.catalog(&input.source, "registry.read").await?;
	crate::registry::system::reject_distribution(&stored)?;
	crate::registry::system::reject_owner_definition(&stored)?;
	scope.require_export(&stored).await?;
	definitions::private_context(scope, &stored).await?;
	let (entity, mut lineage, dependencies) = if let Some(p) = &stored.installation {
		let revision = scope.revision(&p.installation, p.revision).await?;
		let source = load(scope, &revision.source.key, "marketplace.read").await?;
		readable(scope, &source, node).await?;
		// Installed exports preserve the original frozen graph. Extra references
		// cannot silently disappear, or replace dependencies with local overrides.
		if input
			.dependencies
			.iter()
			.any(|r| !source.dependencies.iter().any(|d| &d.reference == r))
		{
			return Err(Error::Forbidden);
		}
		let mut lineage = source.lineage.clone();
		let ancestors: Vec<_> = lineage
			.iter()
			.map(|e| e.source.clone())
			.chain(std::iter::once(source.key.clone()))
			.collect();
		for ancestor in ancestors {
			lineage.insert(ConsentEdge {
				source: ancestor,
				redistributor: tenant.clone(),
				grant: SourceGrant::Consent,
			});
		}
		// Export source bytes, not the installing tenant's configuration or bindings.
		(manifest(&source)?.entity, lineage, source.dependencies)
	} else {
		let deps =
			definitions::publication_graph(scope, &stored, &input.dependencies, node).await?;
		(stored.clone(), BTreeSet::new(), deps)
	};
	if !matches!(entity.kind.as_str(), "agent" | "tool" | "skill" | "bundle") {
		return Err(Error::Invalid(
			"only agent, tool and skill packages can be published".into(),
		));
	}
	// Known copies and supported derivation paths retain the original consent.
	lineage.extend(scope.provenance(&stored).await?);
	for dependency in &dependencies {
		if let Some(key) = &dependency.package {
			let source = load(scope, key, "marketplace.read").await?;
			lineage.extend(source.lineage);
			lineage.insert(ConsentEdge {
				source: key.clone(),
				redistributor: tenant.clone(),
				grant: SourceGrant::Audience,
			});
			lineage.insert(ConsentEdge {
				source: key.clone(),
				redistributor: tenant.clone(),
				grant: SourceGrant::Consent,
			});
		}
	}
	let ancestors: Vec<_> = lineage.iter().map(|e| e.source.clone()).collect();
	for source in ancestors {
		lineage.insert(ConsentEdge {
			source,
			redistributor: tenant.clone(),
			grant: SourceGrant::Consent,
		});
	}
	let package = Package {
		entity,
		author: input.author.clone(),
		permissions: input.permissions.clone(),
		dependencies: dependencies.iter().map(|d| d.reference.clone()).collect(),
	};
	validation.validate_in(&package.entity, false)?;
	let manifest_source = serde_json::to_string(&package)?;
	if manifest_source.len() > 1_048_576 {
		return Err(Error::Invalid("package exceeds one MiB".into()));
	}
	let key = key(&(node, &tenant, &input.package_id, &package.entity.version));
	let version = Version {
		key: key.clone(),
		repository: node.to_owned(),
		owner_tenant: tenant.clone(),
		package_id: input.package_id.clone(),
		version: package.entity.version.clone(),
		kind: package.entity.kind.clone(),
		publisher: scope.actor().to_owned(),
		source: input.source.clone(),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies,
		lineage,
	};
	scope
		.require(&scope.package_resource(&version), "marketplace.publish")
		.await?;
	consent(scope, &version.lineage, &BTreeSet::from([tenant.clone()])).await?;
	Ok(version)
}

pub async fn publish(
	scope: &mut dyn PublicationScope,
	validation: &DefinitionValidation,
	input: &PublishCommand,
	node: &str,
) -> Result<Value> {
	scope.operation(
		"marketplace.publish",
		json!({"source":input.source,"package_id":input.package_id}),
		Some(input.idempotency_key),
	);
	let version = prepare(scope, validation, input, node).await?;
	scope.audit_resource(json!({"key":version.key,"repository":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"digest":version.digest,"source":version.source}));
	let fingerprint = key(input);
	let replay = scope.publication_replay(input.idempotency_key).await?;
	if let Some(saved) = replay {
		// Revalidate the saved disclosure path before exposing either a result
		// or a conflicting-input diagnostic; revocation can hide both.
		let existing = load(
			scope,
			saved.result["key"].as_str().ok_or(Error::Forbidden)?,
			"marketplace.read",
		)
		.await?;
		readable(scope, &existing, node).await?;
		if saved.fingerprint != fingerprint {
			return Err(Error::Conflict(
				"idempotency key has different input".into(),
			));
		}
		if saved.result != json!({"key":version.key,"digest":version.digest}) {
			return Err(conflict());
		}
		return Ok(saved.result);
	}
	if let Some(existing) = scope.version(&version.key).await? {
		load(scope, &version.key, "marketplace.read").await?;
		readable(scope, &existing, node).await?;
		if existing.manifest_source != version.manifest_source
			|| existing.lineage != version.lineage
		{
			return Err(conflict());
		}
	} else {
		let kind = scope.package_kind(node, &input.package_id).await?;
		if kind.is_some_and(|kind| kind != version.kind) {
			return Err(conflict());
		}
		scope
			.insert_version(
				&version,
				&aidash_domain::marketplace::publication::initial_audience(&version),
			)
			.await?;
		scope
			.event(
				"marketplace.published",
				json!({"key":version.key,"tenant":version.owner_tenant}),
			)
			.await?;
	}
	let result = json!({"key":version.key,"digest":version.digest});
	scope
		.remember_publication(input.idempotency_key, fingerprint, result.clone())
		.await?;
	Ok(result)
}

pub async fn share(
	scope: &mut dyn DistributionWriter,
	key: &str,
	redistributor: Option<&str>,
	mut input: AudienceChange,
) -> Result<Audience> {
	if input.tenants.len() > 128
		|| input.expected_revision < 0
		|| input.expected_revision == i64::MAX
	{
		return Err(Error::Invalid("invalid audience revision or size".into()));
	}
	for tenant in &input.tenants {
		identifier(tenant)?;
	}
	let version = scope.version(key).await?.ok_or(Error::Forbidden)?;
	if version.owner_tenant != scope.tenant() {
		return Err(Error::Forbidden);
	}
	let action = if let Some(tenant) = redistributor {
		identifier(tenant)?;
		"marketplace.redistribution.manage"
	} else {
		"marketplace.share"
	};
	scope.operation(
		action,
		json!({"key":key,"digest":version.digest,"redistributor":redistributor}),
		None,
	);
	scope
		.require(&scope.package_resource(&version), action)
		.await?;
	if redistributor.is_none() {
		input.tenants.insert(version.owner_tenant.clone());
		if input.tenants.len() > 128 {
			return Err(Error::Invalid("audience exceeds 128 tenants".into()));
		}
	}
	let previous = scope.distribution(key, redistributor).await?;
	if previous.as_ref().map_or(0, |p| p.revision) != input.expected_revision {
		return Err(conflict());
	}
	if redistributor.is_none() {
		consent(scope, &version.lineage, &input.tenants).await?;
	}
	let next = Audience {
		revision: input.expected_revision + 1,
		tenants: input.tenants,
	};
	scope.save_distribution(key, redistributor, &next).await?;
	scope
		.event(
			"marketplace.distribution_changed",
			json!({"key":key,"tenant":version.owner_tenant,"revision":next.revision}),
		)
		.await?;
	Ok(next)
}

#[cfg(test)]
mod tests;

pub async fn preview(
	scope: &mut dyn PublicationScope,
	validation: &crate::registry::DefinitionValidation,
	input: &PublishCommand,
	node: &str,
) -> Result<aidash_domain::marketplace::PublicationPreview> {
	scope.operation("marketplace.publication_preview", json!({"source":input.source,"package_id":input.package_id.chars().take(256).collect::<String>()}), None);
	match prepare(scope, validation, input, node).await {
		Ok(version) => Ok(aidash_domain::marketplace::PublicationPreview {
			allowed: true,
			version: Some(version.version),
		}),
		Err(Error::Forbidden) => Ok(aidash_domain::marketplace::PublicationPreview {
			allowed: false,
			version: None,
		}),
		Err(e) => Err(e),
	}
}

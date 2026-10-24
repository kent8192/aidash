//! Current audiences, ancestor consents and exact dependency disclosure govern reads.
use super::definitions;
use crate::{Error, Result, ports::marketplace::DistributionScope};
use aidash_domain::marketplace::definitions::{
	content, installation_id, key, manifest, prospective_installation, reference,
};
use aidash_domain::marketplace::*;
use std::collections::BTreeSet;
pub async fn load(scope: &mut dyn DistributionScope, key: &str, action: &str) -> Result<Version> {
	let version: Version = scope.version(key).await?.ok_or(Error::Forbidden)?;
	scope.operation(
		action,
		serde_json::json!({"key":key,"repository":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"digest":version.digest}),
		None,
	);
	scope
		.require(&scope.package_resource(&version), action)
		.await?;
	let recipient = scope.tenant().to_owned();
	distributed(scope, &version, &recipient).await?;
	Ok(version)
}

pub async fn consent(
	scope: &mut dyn DistributionScope,
	edges: &BTreeSet<ConsentEdge>,
	recipients: &BTreeSet<String>,
) -> Result<()> {
	if edges.len() > 128 {
		return Err(Error::Forbidden);
	}
	for edge in edges {
		let source: Version = scope.version(&edge.source).await?.ok_or(Error::Forbidden)?;
		// Source owners do not need their own onward permission, but an ancestor
		// owned by a different tenant always remains a live dependency.
		if source.owner_tenant == edge.redistributor {
			continue;
		}
		if edge.grant == SourceGrant::Audience {
			let audience: Audience = scope
				.audience(&edge.source)
				.await?
				.ok_or(Error::Forbidden)?;
			scope.authority(format!("audience:{}", edge.source), audience.revision);
			if !audience.tenants.contains(&edge.redistributor) {
				return Err(Error::Forbidden);
			}
			continue;
		}
		let grant: Audience = scope
			.consent(&key(&(&edge.source, &edge.redistributor)))
			.await?
			.ok_or(Error::Forbidden)?;
		scope.authority(
			format!("consent:{}:{}", edge.source, edge.redistributor),
			grant.revision,
		);
		if !recipients.is_subset(&grant.tenants) {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}

pub async fn distributed(
	scope: &mut dyn DistributionScope,
	version: &Version,
	recipient: &str,
) -> Result<()> {
	let audience: Audience = scope
		.audience(&version.key)
		.await?
		.ok_or(Error::Forbidden)?;
	scope.authority(format!("audience:{}", version.key), audience.revision);
	if !audience.tenants.contains(recipient) {
		return Err(Error::Forbidden);
	}
	consent(
		scope,
		&version.lineage,
		&BTreeSet::from([recipient.to_string()]),
	)
	.await
}

/// Every serialized reference needs a currently permitted disclosure path.
/// Pending local copies use their management reader; a live published dependency
/// can be read without being installed. The worklist is bounded and cycle-safe.
pub async fn readable(scope: &mut dyn DistributionScope, root: &Version, node: &str) -> Result<()> {
	let mut queue = vec![root.clone()];
	let mut seen = BTreeSet::new();
	while let Some(version) = queue.pop() {
		if !seen.insert(version.key.clone()) {
			continue;
		}
		if seen.len() > 128 {
			return Err(Error::Forbidden);
		}
		let package = manifest(&version)?;
		definitions::private_context(scope, &package.entity).await?;
		for dep in &version.dependencies {
			let local = if let Some(package) = &dep.package {
				let install = scope
					.installation(&installation_id(scope.tenant(), package))
					.await?;
				if let Some(install) = install {
					let revision = scope
						.revision(
							&install.id,
							install.active_revision.unwrap_or(install.latest_revision),
						)
						.await?;
					Some(reference(&revision.entry))
				} else {
					None
				}
			} else {
				Some(dep.reference.clone())
			};
			if let Some(local) = local {
				match definitions::local_graph(scope, vec![(local, dep.kind.clone())], node).await {
					Ok(entries)
						if dep.package.is_some()
							|| entries.first().is_some_and(|e| content(e) == dep.digest) =>
					{
						continue;
					}
					Ok(_) | Err(Error::Forbidden) => {}
					Err(error) => return Err(error),
				}
			}
			let package = dep.package.as_ref().ok_or(Error::Forbidden)?;
			queue.push(load(scope, package, "marketplace.read").await?);
		}
	}
	Ok(())
}

pub async fn summary(
	scope: &mut dyn DistributionScope,
	version: &Version,
	node: &str,
) -> Result<Summary> {
	let package = manifest(version)?;
	let mut actions = vec![];
	let resource = scope.package_resource(version);
	let can_read = scope.decide(&resource, "marketplace.read").await?
		&& match readable(scope, version, node).await {
			Ok(()) => true,
			Err(Error::Forbidden) => false,
			Err(e) => return Err(e),
		};
	if can_read {
		actions.push("read".into());
	}
	if can_read && scope.decide(&resource, "marketplace.install").await? {
		let prospective = prospective_installation(scope.tenant(), &version.key);
		let resource = scope.installation_resource(&prospective, None);
		if scope.decide(&resource, "installation.create").await?
			&& scope.decide(&resource, "installation.read").await?
		{
			actions.push("install".into());
		}
	}
	if scope.tenant() == version.owner_tenant {
		for (action, label) in [
			("marketplace.share", "share"),
			("marketplace.redistribution.manage", "consent"),
		] {
			if scope.decide(&resource, action).await? {
				actions.push(label.into());
			}
		}
	}
	Ok(Summary {
		key: version.key.clone(),
		repository: version.repository.clone(),
		owner_tenant: version.owner_tenant.clone(),
		package_id: version.package_id.clone(),
		version: version.version.clone(),
		kind: version.kind.clone(),
		name: package.entity.name,
		description: package.entity.description,
		author: package.author,
		capabilities: package.entity.capabilities,
		permissions: package.permissions,
		languages: package.entity.languages,
		digest: version.digest.clone(),
		actions,
	})
}

pub async fn detail(scope: &mut dyn DistributionScope, key: &str, node: &str) -> Result<Detail> {
	let version = load(scope, key, "marketplace.read").await?;
	readable(scope, &version, node).await?;
	let summary = summary(scope, &version, node).await?;
	let mut audience: Audience = scope.audience(key).await?.ok_or(Error::Forbidden)?;
	if !summary.actions.iter().any(|a| a == "share") {
		audience.tenants.retain(|t| t == scope.tenant());
	}
	Ok(Detail {
		summary,
		manifest: manifest(&version)?,
		audience,
	})
}

#[cfg(test)]
mod tests;

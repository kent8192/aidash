//! Duty templates in the Marketplace (#1011 D4; decisions 13 and 47 of the
//! agent activation design). A template is a package of its own kind,
//! published, shared, browsed and read like every other package under the
//! same `marketplace.*` actions. It is published from a Duty rather than a
//! Registry definition, and installing one sets up a Duty rather than a
//! Registry copy, so those two steps live here. The Duty side of both is
//! [`crate::duties`]: a use case here never holds a Duty port, because the
//! adapters of both borrow the one transaction of the request.
use super::{
	distribution::{distributed, load, readable},
	publication::record,
};
use crate::{
	Error, Result,
	ports::marketplace::{DistributionScope, MarketplaceRead, PublicationScope},
};
use aidash_domain::{
	duties::templates::{KIND, Listing, Source, Template},
	marketplace::{
		Version,
		definitions::{key, manifest},
	},
	registry::EntityRef,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The largest package, as for every other kind.
const PACKAGE_MAX_BYTES: usize = 1_048_576;

/// How a person lists the template of one of their Duties.
#[derive(Debug, Clone, Serialize)]
pub struct PublishTemplate {
	pub package_id: String,
	/// A semantic version such as `1.0.0`. A later version is offered to
	/// the Duties installed from an earlier one.
	pub version: String,
	pub author: String,
	/// A short description; the start of the mission when empty.
	pub description: String,
	pub idempotency_key: Uuid,
}

/// Publishes `template` as a Marketplace package of the scope's Tenant,
/// shared with that Tenant alone until its owner shares it further. The
/// caller has checked that the person is the Requester of the Duty the
/// template comes from.
pub async fn publish(
	scope: &mut dyn PublicationScope,
	template: &Template,
	input: &PublishTemplate,
	node: &str,
	now: DateTime<Utc>,
) -> Result<Value> {
	scope.operation(
		"marketplace.publish",
		json!({"kind": KIND, "package_id": input.package_id}),
		Some(input.idempotency_key),
	);
	template.validate(now)?;
	let version = prepare(scope, template, input, node)?;
	scope
		.require(&scope.package_resource(&version), "marketplace.publish")
		.await?;
	scope.audit_resource(json!({"key":version.key,"repository":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"digest":version.digest,"kind":KIND}));
	record(
		scope,
		&version,
		key(&(KIND, template, input)),
		input.idempotency_key,
		node,
	)
	.await
}

/// The immutable version that carries `template`. It has no dependencies
/// and no lineage: a template copies nothing another Tenant must consent
/// to. Its source names the package itself, never the Duty it came from.
fn prepare(
	scope: &mut dyn PublicationScope,
	template: &Template,
	input: &PublishTemplate,
	node: &str,
) -> Result<Version> {
	let package = template.package(Listing {
		package_id: &input.package_id,
		version: &input.version,
		author: &input.author,
		description: &input.description,
	})?;
	let manifest_source = serde_json::to_string(&package)?;
	if manifest_source.len() > PACKAGE_MAX_BYTES {
		return Err(Error::Invalid("package exceeds one MiB".into()));
	}
	let tenant = scope.tenant().to_owned();
	Ok(Version {
		key: key(&(node, &tenant, &input.package_id, &package.entity.version)),
		repository: node.to_owned(),
		owner_tenant: tenant,
		package_id: input.package_id.clone(),
		version: package.entity.version.clone(),
		kind: KIND.to_owned(),
		publisher: scope.actor().to_owned(),
		source: EntityRef {
			id: input.package_id.clone(),
			version: package.entity.version.clone(),
		},
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies: vec![],
		lineage: BTreeSet::new(),
	})
}

/// The template version `key` and its template, for the scope's person to
/// install: they may read it (`marketplace.read` and a current audience)
/// and install it (`marketplace.install`), it is a Duty template, its
/// digest is the one they reviewed, and the template passes the checks a
/// published one does, whichever Tenant published it.
pub async fn installable(
	scope: &mut dyn DistributionScope,
	package: &str,
	digest: &str,
	node: &str,
	now: DateTime<Utc>,
) -> Result<(Version, Template)> {
	scope.operation(
		"marketplace.install",
		json!({"package": package, "digest": digest, "kind": KIND}),
		None,
	);
	let version = load(scope, package, "marketplace.read").await?;
	readable(scope, &version, node).await?;
	scope
		.require(&scope.package_resource(&version), "marketplace.install")
		.await?;
	if version.kind != KIND {
		return Err(Error::Invalid(
			"only a Duty template can be installed as a Duty".into(),
		));
	}
	if version.digest != digest {
		return Err(Error::Conflict(
			"revision or immutable content changed".into(),
		));
	}
	let template = Template::from_package(&manifest(&version)?)?;
	template.validate(now)?;
	Ok((version, template))
}

/// The latest version of the template a Duty was installed from that is
/// later than the one it has and that the scope's person may still read
/// and install. Never applied by itself: its Requester chooses to.
pub async fn newer(
	scope: &mut dyn MarketplaceRead,
	source: &Source,
	node: &str,
) -> Result<Option<Source>> {
	let candidates = scope
		.package_versions(&source.repository, &source.owner_tenant, &source.package_id)
		.await?;
	let mut latest: Option<Source> = None;
	for candidate in candidates {
		if !source.superseded_by(&candidate)
			|| latest
				.as_ref()
				.is_some_and(|best| !best.superseded_by(&candidate))
		{
			continue;
		}
		let usable = async {
			let resource = scope.package_resource(&candidate);
			scope.require(&resource, "marketplace.read").await?;
			scope.require(&resource, "marketplace.install").await?;
			let tenant = scope.tenant().to_owned();
			distributed(scope, &candidate, &tenant).await?;
			readable(scope, &candidate, node).await
		}
		.await;
		match usable {
			Ok(()) => latest = Some(Source::of(&candidate, None)),
			Err(Error::Forbidden) => {}
			Err(error) => return Err(error),
		}
	}
	Ok(latest)
}

// Tests: `publication/tests.rs`, which holds the publication fixture.

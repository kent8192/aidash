use super::{definitions, storage::*, *};
use crate::{
	Error, Result,
	authorization::{access::Access, catalog},
	store::Store,
};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::json;
use sha2::{Digest, Sha256};

pub(super) fn manifest(version: &Version) -> Result<Package> {
	if format!(
		"sha256:{:x}",
		Sha256::digest(version.manifest_source.as_bytes())
	) != version.digest
	{
		return Err(conflict());
	}
	let package: Package =
		serde_json::from_str(&version.manifest_source).map_err(|_| conflict())?;
	if package.entity.kind != version.kind
		|| package.entity.version != version.version
		|| package.entity.installation.is_some()
	{
		return Err(conflict());
	}
	Ok(package)
}
pub(super) fn resource(
	access: &Access,
	version: &Version,
) -> crate::authorization::policy::Resource {
	access.resource("package",&version.key,json!({"repository_node":version.repository,"owner_tenant":version.owner_tenant,
		"package_id":version.package_id,"version":version.version,"kind":version.kind,"source_digest":version.digest,"publisher":version.publisher}))
}
pub(super) async fn load(access: &mut Access, key: &str, action: &str) -> Result<Version> {
	let version: Version = get(&mut access.tx, "marketplace_versions", key)
		.await?
		.ok_or(Error::Forbidden)?;
	operation(
		access,
		action,
		serde_json::json!({"key":key,"repository":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"digest":version.digest}),
		None,
	);
	access.require(&resource(access, &version), action).await?;
	distributed(access, &version, &access.identity.tenant.clone()).await?;
	Ok(version)
}
pub(super) async fn consent(
	access: &mut Access,
	edges: &BTreeSet<ConsentEdge>,
	recipients: &BTreeSet<String>,
) -> Result<()> {
	if edges.len() > 128 {
		return Err(Error::Forbidden);
	}
	for edge in edges {
		let source: Version = get(&mut access.tx, "marketplace_versions", &edge.source)
			.await?
			.ok_or(Error::Forbidden)?;
		// Source owners do not need their own onward permission, but an ancestor
		// owned by a different tenant always remains a live dependency.
		if source.owner_tenant == edge.redistributor {
			continue;
		}
		if edge.grant == SourceGrant::Audience {
			let audience: Audience = get(&mut access.tx, "marketplace_audiences", &edge.source)
				.await?
				.ok_or(Error::Forbidden)?;
			authority(
				access,
				format!("audience:{}", edge.source),
				audience.revision,
			);
			if !audience.tenants.contains(&edge.redistributor) {
				return Err(Error::Forbidden);
			}
			continue;
		}
		let grant: Audience = get(
			&mut access.tx,
			"marketplace_consents",
			&key(&(&edge.source, &edge.redistributor)),
		)
		.await?
		.ok_or(Error::Forbidden)?;
		authority(
			access,
			format!("consent:{}:{}", edge.source, edge.redistributor),
			grant.revision,
		);
		if !recipients.is_subset(&grant.tenants) {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}
pub(super) async fn distributed(
	access: &mut Access,
	version: &Version,
	recipient: &str,
) -> Result<()> {
	let audience: Audience = get(&mut access.tx, "marketplace_audiences", &version.key)
		.await?
		.ok_or(Error::Forbidden)?;
	authority(
		access,
		format!("audience:{}", version.key),
		audience.revision,
	);
	if !audience.tenants.contains(recipient) {
		return Err(Error::Forbidden);
	}
	consent(
		access,
		&version.lineage,
		&BTreeSet::from([recipient.to_string()]),
	)
	.await
}
/// Every serialized reference needs a currently permitted disclosure path.
/// Pending local copies use their management reader; a live published dependency
/// can be read without being installed. The worklist is bounded and cycle-safe.
pub(super) async fn readable(access: &mut Access, root: &Version, node: &str) -> Result<()> {
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
		definitions::private_context(access, &package.entity).await?;
		for dep in &version.dependencies {
			let local = if let Some(package) = &dep.package {
				let install: Option<Installation> = get(
					&mut access.tx,
					"marketplace_installations",
					&super::installations::id(&access.identity.tenant, package),
				)
				.await?;
				if let Some(install) = install {
					let revision = super::installations::revision(
						&mut access.tx,
						&install.id,
						install.latest_revision,
					)
					.await?;
					Some(definitions::reference(&revision.entry))
				} else {
					None
				}
			} else {
				Some(dep.reference.clone())
			};
			if let Some(local) = local {
				match definitions::local_graph(access, vec![(local, dep.kind.clone())], node).await
				{
					Ok(entries)
						if dep.package.is_some()
							|| entries
								.first()
								.is_some_and(|e| definitions::content(e) == dep.digest) =>
					{
						continue;
					}
					Ok(_) | Err(Error::Forbidden) => {}
					Err(error) => return Err(error),
				}
			}
			let package = dep.package.as_ref().ok_or(Error::Forbidden)?;
			queue.push(load(access, package, "marketplace.read").await?);
		}
	}
	Ok(())
}
pub(super) async fn summary(access: &mut Access, version: &Version, node: &str) -> Result<Summary> {
	let package = manifest(version)?;
	let mut actions = vec![];
	let resource = resource(access, version);
	let can_read = access.decide(&resource, "marketplace.read").await?
		&& match readable(access, version, node).await {
			Ok(()) => true,
			Err(Error::Forbidden) => false,
			Err(e) => return Err(e),
		};
	if can_read {
		actions.push("read".into());
	}
	if can_read && access.decide(&resource, "marketplace.install").await? {
		let prospective = super::installations::prospective(&access.identity.tenant, &version.key);
		let resource = super::installations::resource(access, &prospective, None);
		if access.decide(&resource, "installation.create").await?
			&& access.decide(&resource, "installation.read").await?
		{
			actions.push("install".into());
		}
	}
	if access.identity.tenant == version.owner_tenant {
		for (action, label) in [
			("marketplace.share", "share"),
			("marketplace.redistribution.manage", "consent"),
		] {
			if access.decide(&resource, action).await? {
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
pub(super) async fn detail(access: &mut Access, key: &str, node: &str) -> Result<Detail> {
	let version = load(access, key, "marketplace.read").await?;
	readable(access, &version, node).await?;
	let summary = summary(access, &version, node).await?;
	let mut audience: Audience = get(&mut access.tx, "marketplace_audiences", key)
		.await?
		.ok_or(Error::Forbidden)?;
	if !summary.actions.iter().any(|a| a == "share") {
		audience.tenants.retain(|t| t == &access.identity.tenant);
	}
	Ok(Detail {
		summary,
		manifest: manifest(&version)?,
		audience,
	})
}
pub(super) async fn prepare(
	store: &Store,
	access: &mut Access,
	input: &Publish,
) -> Result<Version> {
	crate::authorization::policy::identifier(&input.package_id)?;
	if input.author.trim().is_empty()
		|| input.author.len() > 256
		|| input.dependencies.len() > 128
		|| input.permissions.len() > 128
	{
		return Err(Error::Invalid("invalid publication metadata".into()));
	}
	let stored = catalog::entry(access, &input.source, "registry.read").await?;
	access
		.require(&catalog::resource(access, &stored), "registry.export")
		.await?;
	definitions::private_context(access, &stored).await?;
	let (entity, mut lineage, dependencies) = if let Some(p) = &stored.installation {
		let revision =
			super::installations::revision(&mut access.tx, &p.installation, p.revision).await?;
		let source = load(access, &revision.source.key, "marketplace.read").await?;
		readable(access, &source, &store.node_id).await?;
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
				redistributor: access.identity.tenant.clone(),
				grant: SourceGrant::Consent,
			});
		}
		// Export source bytes, not the installing tenant's configuration or bindings.
		(manifest(&source)?.entity, lineage, source.dependencies)
	} else {
		let deps =
			definitions::publication_graph(access, &stored, &input.dependencies, &store.node_id)
				.await?;
		(stored.clone(), BTreeSet::new(), deps)
	};
	if !matches!(entity.kind.as_str(), "agent" | "tool" | "skill") {
		return Err(Error::Invalid(
			"only agent, tool and skill packages can be published".into(),
		));
	}
	// Known copies and supported derivation paths retain the original consent.
	lineage.extend(
		super::installations::provenance(&mut access.tx, &stored, &access.identity.tenant).await?,
	);
	for dependency in &dependencies {
		if let Some(key) = &dependency.package {
			let source = load(access, key, "marketplace.read").await?;
			lineage.extend(source.lineage);
			lineage.insert(ConsentEdge {
				source: key.clone(),
				redistributor: access.identity.tenant.clone(),
				grant: SourceGrant::Audience,
			});
			lineage.insert(ConsentEdge {
				source: key.clone(),
				redistributor: access.identity.tenant.clone(),
				grant: SourceGrant::Consent,
			});
		}
	}
	let ancestors: Vec<_> = lineage.iter().map(|e| e.source.clone()).collect();
	for source in ancestors {
		lineage.insert(ConsentEdge {
			source,
			redistributor: access.identity.tenant.clone(),
			grant: SourceGrant::Consent,
		});
	}
	let package = Package {
		entity,
		author: input.author.clone(),
		permissions: input.permissions.clone(),
		dependencies: dependencies.iter().map(|d| d.reference.clone()).collect(),
	};
	crate::registry::validate_structure(&package.entity)?;
	let manifest_source = serde_json::to_string(&package)?;
	if manifest_source.len() > 1_048_576 {
		return Err(Error::Invalid("package exceeds one MiB".into()));
	}
	let key = key(&(
		&store.node_id,
		&access.identity.tenant,
		&input.package_id,
		&package.entity.version,
	));
	let version = Version {
		key: key.clone(),
		repository: store.node_id.clone(),
		owner_tenant: access.identity.tenant.clone(),
		package_id: input.package_id.clone(),
		version: package.entity.version.clone(),
		kind: package.entity.kind.clone(),
		publisher: access.identity.subject.clone(),
		source: input.source.clone(),
		digest: format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())),
		manifest_source,
		dependencies,
		lineage,
	};
	access
		.require(&resource(access, &version), "marketplace.publish")
		.await?;
	consent(
		access,
		&version.lineage,
		&BTreeSet::from([access.identity.tenant.clone()]),
	)
	.await?;
	Ok(version)
}
pub(super) async fn publish(store: &Store, access: &mut Access, input: &Publish) -> Result<Value> {
	operation(
		access,
		"marketplace.publish",
		json!({"source":input.source,"package_id":input.package_id}),
		Some(input.idempotency_key),
	);
	let version = prepare(store, access, input).await?;
	if let Some(audit) = &mut access.marketplace_audit {
		audit["resource"] = json!({"key":version.key,"repository":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"digest":version.digest,"source":version.source});
	}
	let key = version.key.clone();
	let fingerprint = super::storage::key(input);
	let replay = replay(
		access,
		"publish",
		input.idempotency_key,
		&fingerprint,
		&store.node_id,
	)
	.await?;
	if let Some(saved) = &replay {
		let existing = load(
			access,
			saved["key"].as_str().ok_or(Error::Forbidden)?,
			"marketplace.read",
		)
		.await?;
		readable(access, &existing, &store.node_id).await?;
		if saved != &json!({"key":version.key,"digest":version.digest}) {
			return Err(conflict());
		}
		return Ok(saved.clone());
	}
	let existing: Option<Version> = get(&mut access.tx, "marketplace_versions", &key).await?;
	if let Some(existing) = existing {
		load(access, &key, "marketplace.read").await?;
		readable(access, &existing, &store.node_id).await?;
		if existing.manifest_source != version.manifest_source
			|| existing.lineage != version.lineage
		{
			return Err(conflict());
		}
	} else {
		let kind: Option<String> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("kind"))
				.from(Alias::new("marketplace_versions"))
				.and_where(Expr::cust("repository=$1 AND owner=$2 AND package_id=$3"))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&store.node_id)
		.bind(&access.identity.tenant)
		.bind(&input.package_id)
		.fetch_optional(&mut **access.tx)
		.await?;
		if kind.is_some_and(|kind| kind != version.kind) {
			return Err(conflict());
		}
		insert_in(&mut access.tx, &version).await?;
		store
			.event(
				&mut access.tx,
				None,
				"marketplace.published",
				json!({"key":version.key,"tenant":version.owner_tenant}),
			)
			.await?;
	}
	let result = json!({"key":version.key,"digest":version.digest});
	if let Some(replay) = replay
		&& replay != result
	{
		return Err(conflict());
	}
	remember(
		access,
		"publish",
		input.idempotency_key,
		fingerprint,
		result.clone(),
	)
	.await?;
	Ok(result)
}
pub(super) async fn insert_in(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	version: &Version,
) -> Result<()> {
	let source_content = definitions::content(&manifest(version)?.entity);
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("marketplace_versions"))
			.columns(
				[
					"key",
					"document",
					"repository",
					"owner",
					"package_id",
					"version",
					"kind",
					"source_id",
					"source_version",
					"source_content",
				]
				.map(Alias::new),
			)
			.values_panic((1..=10).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&version.key)
	.bind(json!(version))
	.bind(&version.repository)
	.bind(&version.owner_tenant)
	.bind(&version.package_id)
	.bind(&version.version)
	.bind(&version.kind)
	.bind(&version.source.id)
	.bind(&version.source.version)
	.bind(source_content)
	.execute(&mut **tx)
	.await?;
	put(
		tx,
		"marketplace_audiences",
		&version.key,
		&Audience {
			revision: 1,
			tenants: BTreeSet::from([version.owner_tenant.clone()]),
		},
	)
	.await
}
pub(super) async fn share(
	store: &Store,
	access: &mut Access,
	key: &str,
	redistributor: Option<&str>,
	input: AudienceInput,
) -> Result<Audience> {
	if input.tenants.len() > 128
		|| input.expected_revision < 0
		|| input.expected_revision == i64::MAX
	{
		return Err(Error::Invalid("invalid audience revision or size".into()));
	}
	for tenant in &input.tenants {
		crate::authorization::policy::identifier(tenant)?;
	}
	let version: Version = get(&mut access.tx, "marketplace_versions", key)
		.await?
		.ok_or(Error::Forbidden)?;
	if version.owner_tenant != access.identity.tenant {
		return Err(Error::Forbidden);
	}
	let (table, target, action) = if let Some(tenant) = redistributor {
		crate::authorization::policy::identifier(tenant)?;
		(
			"marketplace_consents",
			super::storage::key(&(key, tenant)),
			"marketplace.redistribution.manage",
		)
	} else {
		(
			"marketplace_audiences",
			key.to_string(),
			"marketplace.share",
		)
	};
	operation(
		access,
		action,
		json!({"key":key,"digest":version.digest,"redistributor":redistributor}),
		None,
	);
	access.require(&resource(access, &version), action).await?;
	let previous: Option<Audience> = get(&mut access.tx, table, &target).await?;
	if previous.as_ref().map_or(0, |p| p.revision) != input.expected_revision {
		return Err(conflict());
	}
	if redistributor.is_none() {
		consent(access, &version.lineage, &input.tenants).await?;
	}
	let next = Audience {
		revision: input.expected_revision + 1,
		tenants: input.tenants,
	};
	put(&mut access.tx, table, &target, &next).await?;
	authority(access, format!("{table}:{target}"), next.revision);
	store
		.event(
			&mut access.tx,
			None,
			"marketplace.distribution_changed",
			json!({"key":key,"tenant":version.owner_tenant,"revision":next.revision}),
		)
		.await?;
	Ok(next)
}

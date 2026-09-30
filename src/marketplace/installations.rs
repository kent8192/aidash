use super::{definitions, distribution, storage::*, *};
use crate::{
	Error, Result,
	authorization::{access::Access, policy::Resource},
	store::Store,
};
use sea_orm::sea_query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};

pub(super) fn id(tenant: &str, package: &str) -> String {
	key(&(tenant, package))
}
pub(super) fn prospective(tenant: &str, package: &str) -> Installation {
	Installation {
		id: id(tenant, package),
		tenant: tenant.into(),
		package_key: package.into(),
		latest_revision: 0,
		active_revision: None,
		activation_revision: 0,
	}
}
pub(super) fn resource(access: &Access, install: &Installation, revision: Option<i64>) -> Resource {
	access.resource("installation",&install.id,json!({"installing_tenant":install.tenant,"package_key":install.package_key,"installation_revision":revision}))
}
pub(super) async fn revision(
	tx: &mut Transaction<'_, Postgres>,
	id: &str,
	revision: i64,
) -> Result<Revision> {
	get(tx, "marketplace_revisions", &key(&(id, revision)))
		.await?
		.ok_or(Error::Forbidden)
}
pub(super) async fn owned(access: &mut Access, id: &str) -> Result<Installation> {
	let install: Installation = get(&mut access.tx, "marketplace_installations", id)
		.await?
		.ok_or(Error::Forbidden)?;
	if install.tenant != access.identity.tenant {
		return Err(Error::Forbidden);
	}
	access
		.require(&resource(access, &install, None), "installation.read")
		.await?;
	Ok(install)
}
pub(super) async fn view(
	access: &mut Access,
	id: &str,
	rev: Option<i64>,
	node: &str,
) -> Result<InstallationRevision> {
	operation(
		access,
		"installation.read",
		json!({"installation":id,"revision":rev}),
		None,
	);
	let install = owned(access, id).await?;
	let revision = revision(&mut access.tx, id, rev.unwrap_or(install.latest_revision)).await?;
	access
		.require(
			&resource(access, &install, Some(revision.revision)),
			"installation.read",
		)
		.await?;
	definitions::local_graph(
		access,
		revision
			.dependencies
			.iter()
			.cloned()
			.map(|r| (r, String::new()))
			.collect(),
		node,
	)
	.await?;
	definitions::private_context(access, &revision.entry).await?;
	if let Some(audit) = &mut access.marketplace_audit {
		audit["installation"] = json!({"id":id,"revision":revision.revision,"digest":revision.digest,"entry":definitions::reference(&revision.entry)});
	}
	let approved = approved(
		&mut access.tx,
		&install.tenant,
		&definitions::reference(&revision.entry),
	)
	.await?;
	let actions = if access
		.decide(
			&resource(access, &install, Some(revision.revision)),
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
pub(super) async fn install(
	store: &Store,
	access: &mut Access,
	package: &str,
	input: &Install,
) -> Result<InstallationRevision> {
	operation(
		access,
		"marketplace.install",
		json!({"package":package,"digest":input.digest,"installation":id(&access.identity.tenant,package)}),
		Some(input.idempotency_key),
	);
	let source = distribution::load(access, package, "marketplace.read").await?;
	distribution::readable(access, &source, &store.node_id).await?;
	access
		.require(
			&distribution::resource(access, &source),
			"marketplace.install",
		)
		.await?;
	let install = prospective(&access.identity.tenant, package);
	for action in ["installation.create", "installation.read"] {
		access
			.require(&resource(access, &install, None), action)
			.await?;
	}
	if source.digest != input.digest {
		return Err(conflict());
	}
	let fingerprint = key(&(package, input));
	if let Some(saved) = replay(
		access,
		"install",
		input.idempotency_key,
		&fingerprint,
		&store.node_id,
	)
	.await?
	{
		return view(
			access,
			&install.id,
			saved["revision"].as_i64(),
			&store.node_id,
		)
		.await;
	}
	let resolved = definitions::resolve(
		access,
		&source,
		&input.config,
		&input.bindings,
		&store.node_id,
	)
	.await?;
	if let Some(existing) =
		get::<Installation>(&mut access.tx, "marketplace_installations", &install.id).await?
	{
		let current = revision(&mut access.tx, &install.id, existing.latest_revision).await?;
		if !same(&current, &resolved.0, &input.config, &resolved.2) {
			return Err(conflict());
		}
		remember(
			access,
			"install",
			input.idempotency_key,
			fingerprint,
			json!({"installation":install.id,"revision":current.revision}),
		)
		.await?;
		return view(access, &install.id, Some(current.revision), &store.node_id).await;
	}
	let revision = stage(
		store,
		access,
		install,
		&source,
		input.config.clone(),
		resolved,
	)
	.await?;
	remember(
		access,
		"install",
		input.idempotency_key,
		fingerprint,
		json!({"installation":id(&access.identity.tenant, package),"revision":revision}),
	)
	.await?;
	view(
		access,
		&id(&access.identity.tenant, package),
		Some(revision),
		&store.node_id,
	)
	.await
}
fn same(
	current: &Revision,
	candidate: &Entry,
	config: &Value,
	bindings: &[DependencyBinding],
) -> bool {
	definitions::content(&current.entry) == definitions::content(candidate)
		&& current.config == *config
		&& key(&current.bindings) == key(&bindings)
}
pub(super) async fn configure(
	store: &Store,
	access: &mut Access,
	id: &str,
	input: &Configure,
) -> Result<InstallationRevision> {
	if input.expected_revision < 1 || input.expected_revision == i64::MAX {
		return Err(Error::Invalid("invalid installation revision".into()));
	}
	operation(
		access,
		"installation.configure",
		json!({"installation":id,"expected_revision":input.expected_revision}),
		Some(input.idempotency_key),
	);
	let install = owned(access, id).await?;
	access
		.require(
			&resource(access, &install, Some(install.latest_revision)),
			"installation.configure",
		)
		.await?;
	let current = revision(&mut access.tx, id, install.latest_revision).await?;
	let fingerprint = key(&(id, input));
	if let Some(saved) = replay(
		access,
		"configure",
		input.idempotency_key,
		&fingerprint,
		&store.node_id,
	)
	.await?
	{
		return view(access, id, saved["revision"].as_i64(), &store.node_id).await;
	}
	let resolved = definitions::resolve(
		access,
		&current.source,
		&input.config,
		&input.bindings,
		&store.node_id,
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
			store,
			access,
			install,
			&current.source,
			input.config.clone(),
			resolved,
		)
		.await?
	};
	remember(
		access,
		"configure",
		input.idempotency_key,
		fingerprint,
		json!({"installation":id,"revision":next}),
	)
	.await?;
	view(access, id, Some(next), &store.node_id).await
}
pub(super) async fn stage(
	store: &Store,
	access: &mut Access,
	install: Installation,
	source: &Version,
	config: Value,
	resolved: (Entry, Vec<EntityRef>, Vec<DependencyBinding>),
) -> Result<i64> {
	stage_in(
		store,
		&mut access.tx,
		&access.identity.subject,
		install,
		source,
		config,
		resolved,
	)
	.await
}
async fn stage_in(
	store: &Store,
	tx: &mut Transaction<'_, Postgres>,
	actor: &str,
	mut install: Installation,
	source: &Version,
	config: Value,
	resolved: (Entry, Vec<EntityRef>, Vec<DependencyBinding>),
) -> Result<i64> {
	let (mut entry, dependencies, bindings) = resolved;
	install.latest_revision += 1;
	let revision_number = install.latest_revision;
	// Allocate once in this transaction; the persisted revision/idempotency
	// mapping supplies stable retries without claiming any legacy ID namespace.
	entry.id = format!("mkt-{}", Uuid::new_v4().simple());
	// Original semantic version is retained separately in the source. A short
	// local version also keeps qualified Agent identities within 256 bytes.
	entry.version = "1.0.0".into();
	entry.installation = Some(Projection {
		contract: 1,
		tenant: install.tenant.clone(),
		installation: install.id.clone(),
		revision: revision_number,
	});
	crate::registry::validate_structure(&entry)?;
	let revision = Revision {
		installation: install.id.clone(),
		tenant: install.tenant.clone(),
		revision: revision_number,
		digest: key(&entry),
		entry: entry.clone(),
		config,
		dependencies,
		bindings,
		source: source.clone(),
	};
	if revision_number == 1 {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("marketplace_installations"))
				.columns(["key", "document", "tenant", "package_key"].map(Alias::new))
				.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&install.id)
		.bind(json!(install))
		.bind(&install.tenant)
		.bind(&install.package_key)
		.execute(&mut **tx)
		.await?;
	} else {
		put(tx, "marketplace_installations", &install.id, &install).await?;
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("registry"))
			.columns(["id", "version", "kind", "metadata"].map(Alias::new))
			.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.bind(&entry.kind)
	.bind(json!(entry))
	.execute(&mut **tx)
	.await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("marketplace_revisions"))
			.columns(
				[
					"key",
					"document",
					"installation",
					"revision",
					"entry_id",
					"entry_version",
				]
				.map(Alias::new),
			)
			.values_panic((1..=6).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(key(&(&install.id, revision_number)))
	.bind(json!(revision))
	.bind(&install.id)
	.bind(revision_number)
	.bind(&entry.id)
	.bind(&entry.version)
	.execute(&mut **tx)
	.await?;
	if entry.kind == "agent"
		&& entry
			.config
			.get("knowledge_digest")
			.is_some_and(|d| !d.is_null())
	{
		let original = distribution::manifest(source)?.entity;
		let documents: Value = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("documents"))
				.from(Alias::new("agent_knowledge"))
				.and_where(Expr::cust("agent_id=$1 AND agent_version=$2"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&original.id)
		.bind(&original.version)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or(Error::Forbidden)?;
		if entry.config["knowledge_digest"].as_str()
			!= Some(crate::knowledge::digest(&documents).as_str())
		{
			return Err(Error::Forbidden);
		}
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_knowledge"))
				.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
				.values_panic(["$1", "$2", "$3"].map(Expr::cust))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&entry.id)
		.bind(&entry.version)
		.bind(documents)
		.execute(&mut **tx)
		.await?;
	}

	let mut provenance = source.lineage.clone();
	for grant in [SourceGrant::Consent, SourceGrant::Audience] {
		provenance.insert(ConsentEdge {
			source: source.key.clone(),
			redistributor: install.tenant.clone(),
			grant,
		});
	}
	put(
		tx,
		"marketplace_provenance",
		&key(&definitions::reference(&entry)),
		&provenance,
	)
	.await?;
	// Exact-content copies also retain known provenance even if their display ID
	// changes. This is provenance tracking, not a semantic DLP claim.
	let content_key = format!(
		"content-{}",
		key(&(&install.tenant, definitions::content(&entry)))
	);
	let mut copies: BTreeSet<ConsentEdge> = get(tx, "marketplace_provenance", &content_key)
		.await?
		.unwrap_or_default();
	copies.extend(provenance);
	put(tx, "marketplace_provenance", &content_key, &copies).await?;
	store.event(tx,None,"marketplace.installed",json!({"installation":install.id,"tenant":install.tenant,"revision":revision_number,"digest":revision.digest,"actor":actor})).await?;
	Ok(revision_number)
}
pub(super) async fn provenance(
	tx: &mut Transaction<'_, Postgres>,
	entry: &Entry,
	tenant: &str,
) -> Result<BTreeSet<ConsentEdge>> {
	let mut edges: BTreeSet<ConsentEdge> = get(
		tx,
		"marketplace_provenance",
		&key(&definitions::reference(entry)),
	)
	.await?
	.unwrap_or_default();
	let copies: BTreeSet<ConsentEdge> = get(
		tx,
		"marketplace_provenance",
		&format!("content-{}", key(&(tenant, definitions::content(entry)))),
	)
	.await?
	.unwrap_or_default();
	edges.extend(copies);
	Ok(edges)
}
pub(crate) async fn propagate_provenance(
	tx: &mut Transaction<'_, Postgres>,
	source: &EntityRef,
	target: &Entry,
	tenant: &str,
) -> Result<()> {
	let source = definitions::raw(tx, source).await?;
	let edges = provenance(tx, &source, tenant).await?;
	if !edges.is_empty() {
		put(
			tx,
			"marketplace_provenance",
			&key(&definitions::reference(target)),
			&edges,
		)
		.await?;
	}
	Ok(())
}
pub(crate) async fn catalog_owner(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	r: &EntityRef,
	_enabled: bool,
) -> Result<()> {
	let entry = definitions::raw(tx, r).await?;
	if let Some(p) = entry.installation {
		if p.tenant != tenant || p.contract != 1 {
			return Err(Error::Forbidden);
		}
		// Projection approvals are managed only by Marketplace activation, which
		// validates the pinned graph and advances the active revision atomically.
		return Err(Error::Forbidden);
	}
	Ok(())
}
/// Only new discovery/admission requires the active pointer. Runtime reads keep
/// using exact immutable references and their current individual catalog grants.
pub(crate) async fn active(access: &mut Access, entry: &Entry) -> Result<bool> {
	let Some(p) = &entry.installation else {
		return Ok(true);
	};
	if p.tenant != access.identity.tenant || p.contract != 1 {
		return Ok(false);
	}
	// Retain the shared gate lease until admission commits. Catalog readers
	// acquire it before row locks; inherited transactions use their outer lease.
	if !access.inherited_lease {
		lock(&mut access.tx, false).await?;
	}
	match gate(&mut access.tx).await {
		Ok(()) => {}
		Err(Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	}
	let doc: Option<Value> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("document"))
			.from(Alias::new("marketplace_installations"))
			.and_where(Expr::cust("key=$1"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&p.installation)
	.fetch_optional(&mut **access.tx)
	.await?;
	let Some(doc) = doc else {
		return Ok(false);
	};
	let install: Installation = serde_json::from_value(doc)?;
	Ok(install.tenant == p.tenant && install.active_revision == Some(p.revision))
}
pub(super) async fn approved(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	entry: &EntityRef,
) -> Result<bool> {
	let approved: Option<bool> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("enabled"))
			.from(Alias::new("authorization_catalog"))
			.and_where(Expr::cust("tenant=$1 AND entry_id=$2 AND entry_version=$3"))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(&entry.id)
	.bind(&entry.version)
	.fetch_optional(&mut **tx)
	.await?;
	Ok(approved == Some(true))
}
pub(super) async fn activate(store: &Store, id: &str, input: Activate) -> Result<Installation> {
	let mut tx = store.pool.begin().await?;
	crate::authorization::Authorization::load(&mut tx, &input.tenant).await?;
	lock(&mut tx, true).await?;
	if input.enabled {
		gate(&mut tx).await?;
	}
	writer(&mut tx).await?;
	let mut install: Installation = get(&mut tx, "marketplace_installations", id)
		.await?
		.ok_or(Error::Forbidden)?;
	if install.tenant != input.tenant {
		return Err(Error::Forbidden);
	}
	if install.activation_revision != input.expected_activation_revision {
		return Err(conflict());
	}
	let revision = revision(&mut tx, id, input.revision).await?;
	if input.enabled {
		for dep in &revision.dependencies {
			if !approved(&mut tx, &input.tenant, dep).await? {
				return Err(Error::Forbidden);
			}
		}
	}
	crate::authorization::catalog::set_in(
		&mut tx,
		&input.tenant,
		&definitions::reference(&revision.entry),
		input.expected_catalog_revision,
		input.enabled,
		"operator",
	)
	.await?;
	if input.enabled {
		install.active_revision = Some(input.revision);
	}
	// Revoking the active approval leaves the selected pointer in place. It
	// cannot silently fall back to another revision whose old grant still exists.
	install.activation_revision += 1;
	put(&mut tx, "marketplace_installations", id, &install).await?;
	store.event(&mut tx,None,"marketplace.activation_changed",json!({"installation":id,"tenant":input.tenant,"revision":input.revision,"enabled":input.enabled,"actor":"operator"})).await?;
	tx.commit().await?;
	Ok(install)
}

/// Explicit operator recovery copies legacy bytes/configuration into a new
/// pending tenant revision. It neither claims nor mutates the legacy overlay.
pub(super) async fn adopt(store: &Store, input: Adopt) -> Result<Installation> {
	crate::authorization::policy::identifier(&input.tenant)?;
	let mut tx = store.pool.begin().await?;
	crate::authorization::Authorization::load(&mut tx, &input.tenant).await?;
	lock(&mut tx, true).await?;
	gate(&mut tx).await?;
	writer(&mut tx).await?;
	let scope = key(&("operator-adoption", &input.tenant, input.idempotency_key));
	let fingerprint = key(&input);
	if let Some(saved) = get::<Value>(&mut tx, "marketplace_requests", &scope).await? {
		if saved["fingerprint"] != fingerprint {
			return Err(conflict());
		}
		let install = get(
			&mut tx,
			"marketplace_installations",
			saved["id"].as_str().ok_or(Error::Forbidden)?,
		)
		.await?
		.ok_or(Error::Forbidden)?;
		tx.commit().await?;
		return Ok(install);
	}
	let raw = definitions::raw(&mut tx, &input.source).await?;
	if raw.installation.is_some() {
		return Err(Error::Invalid("adoption requires a legacy source".into()));
	}
	let effective =
		crate::registry::effective_in(&mut tx, &input.source.id, &input.source.version).await?;
	let record: Option<(String, String)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("manifest_source"), Alias::new("digest")])
			.from(Alias::new("packages"))
			.and_where(Expr::cust("id=$1 AND version=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(&input.source.id)
	.bind(&input.source.version)
	.fetch_optional(&mut *tx)
	.await?;
	let (manifest_source, digest) = record.ok_or(Error::Forbidden)?;
	let mut package: crate::registry::Package = serde_json::from_str(&manifest_source)?;
	if package.entity != raw {
		return Err(conflict());
	}
	if format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes())) != digest {
		return Err(conflict());
	}
	// Adoption freezes the executable overlay, so its graph and manifest must
	// describe the same references as the staged definition.
	let mut queue = definitions::refs(&effective, &store.node_id)?;
	let mut seen = BTreeSet::new();
	let mut deps = vec![];
	while let Some((reference, kind)) = queue.pop() {
		let entry = definitions::raw(&mut tx, &reference).await?;
		let effective_dependency =
			crate::registry::effective_in(&mut tx, &reference.id, &reference.version).await?;
		let frozen_refs = definitions::refs(&entry, &store.node_id)?;
		let effective_refs = definitions::refs(&effective_dependency, &store.node_id)?;
		if frozen_refs != effective_refs {
			// A transitive legacy overlay cannot be frozen into this root revision.
			return Err(Error::Forbidden);
		}
		if !kind.is_empty() && entry.kind != kind {
			return Err(Error::Forbidden);
		}
		if !seen.insert((reference.id.clone(), reference.version.clone())) {
			continue;
		}
		if seen.len() > 128
			|| entry
				.installation
				.as_ref()
				.is_some_and(|p| p.tenant != input.tenant)
			|| !approved(&mut tx, &input.tenant, &reference).await?
		{
			return Err(Error::Forbidden);
		}
		queue.extend(effective_refs);
		deps.push(Dependency {
			reference,
			kind: entry.kind.clone(),
			digest: definitions::content(&entry),
			package: None,
		});
	}
	package.entity = effective.clone();
	package.dependencies = deps.iter().map(|d| d.reference.clone()).collect();
	let manifest_source = serde_json::to_string(&package)?;
	if manifest_source.len() > 1_048_576 {
		return Err(Error::Invalid("package exceeds one MiB".into()));
	}
	let digest = format!("sha256:{:x}", Sha256::digest(manifest_source.as_bytes()));
	let source_key = key(&(&store.node_id, &input.tenant, &raw.id, &raw.version));
	let source = Version {
		key: source_key.clone(),
		repository: store.node_id.clone(),
		owner_tenant: input.tenant.clone(),
		package_id: raw.id.clone(),
		version: raw.version.clone(),
		kind: raw.kind.clone(),
		publisher: "operator-adoption".into(),
		source: input.source.clone(),
		manifest_source,
		digest,
		dependencies: deps,
		lineage: provenance(&mut tx, &raw, &input.tenant).await?,
	};
	distribution::manifest(&source)?;
	// Legacy adoption cannot erase already-known imported provenance.
	if !source.lineage.is_empty() {
		return Err(Error::Forbidden);
	}
	if let Some(existing) = get::<Version>(&mut tx, "marketplace_versions", &source_key).await? {
		if existing.manifest_source != source.manifest_source
			|| existing.publisher != "operator-adoption"
		{
			return Err(conflict());
		}
	} else {
		distribution::insert_in(&mut tx, &source).await?;
	}
	let install = prospective(&input.tenant, &source_key);
	if get::<Installation>(&mut tx, "marketplace_installations", &install.id)
		.await?
		.is_none()
	{
		let dependencies = source
			.dependencies
			.iter()
			.map(|d| d.reference.clone())
			.collect();
		let config: Option<Value> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("config"))
				.from(Alias::new("installations"))
				.and_where(Expr::cust("id=$1 AND version=$2"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&input.source.id)
		.bind(&input.source.version)
		.fetch_optional(&mut *tx)
		.await?;
		let config = config.unwrap_or_else(|| json!({}));
		stage_in(
			store,
			&mut tx,
			"operator-adoption",
			install.clone(),
			&source,
			config,
			(effective, dependencies, vec![]),
		)
		.await?;
	}
	put(
		&mut tx,
		"marketplace_requests",
		&scope,
		&json!({"fingerprint":fingerprint,"id":install.id}),
	)
	.await?;
	let install = get(&mut tx, "marketplace_installations", &install.id)
		.await?
		.ok_or(Error::Forbidden)?;
	tx.commit().await?;
	Ok(install)
}

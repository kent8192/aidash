//! Consume File publications under a Workspace lock, retaining saved authority.
use super::{access::Lease, mutations::Entries, storage};
use crate::{Error, Result, database::native, store::Store};
use aidash_application::{
	ports::semantic::{
		image_publications::{ImagePublication, ImagePublicationSession},
		mutations::SemanticEntriesSession,
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::semantic::{
	Source,
	indexing::{ImageSlot, ImageSlots, IndexingSpec},
	mutations::Entry,
};
use async_trait::async_trait;
use chrono::Utc;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, Func, JoinType, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::SemanticMaintenance);

/// Serialize image publication and rebuild within one Workspace only.
///
/// Every caller takes the parent `workspaces` row before the advisory lock.
/// Publication inserts need a key-share lock on that row through their foreign
/// key, while configuration and bootstrap lock it `FOR UPDATE` first; taking
/// the row here keeps one order and avoids a lock cycle. A caller already
/// holding `FOR UPDATE` keeps it.
pub(crate) async fn lock_workspace(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	workspace: Uuid,
) -> Result<()> {
	let parent = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("workspaces"))
		.and_where(Expr::col("id").eq(Expr::value(workspace)))
		.lock(LockType::KeyShare)
		.to_string(PostgresQueryBuilder);
	native::query(&parent).execute(&mut *tx).await?;
	let statement = Query::select()
		.expr(Expr::cust(format!(
			"pg_advisory_xact_lock(71003201, hashtext('{workspace}'::text))"
		)))
		.to_string(PostgresQueryBuilder);
	native::query(&statement).execute(tx).await?;
	Ok(())
}

struct Session {
	lease: Option<Lease<'static>>,
	workspace: Uuid,
	index: Option<crate::apps::knowledge::serializers::contracts::Index>,
	authorities: Value,
}

impl Session {
	fn lease(&mut self) -> Result<&mut Lease<'static>> {
		self.lease
			.as_mut()
			.ok_or_else(|| Error::Conflict("image publication transaction ended".into()))
	}
}

fn key(slot: &ImageSlot) -> String {
	format!(
		"working-image:{}:{}:{}:{}:{}",
		slot.area_id, slot.generation, slot.file_id, slot.publication, slot.position
	)
}

fn identity(slot: &ImageSlot) -> (Uuid, i64, Uuid, i64, usize) {
	(
		slot.area_id,
		slot.generation,
		slot.file_id,
		slot.publication,
		slot.position,
	)
}

fn waiting_for_owner(authority: &Value) -> bool {
	authority.get("credential").is_none_or(Value::is_null)
}

async fn owner_authority(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	area: Uuid,
	tenant: &str,
	owner: &str,
) -> Result<Option<Value>> {
	let grant = native::query(&Query::select()
		.columns([("authorization_execution", "credential_id"), ("authorization_execution", "subject_chain")])
		.from_as(Alias::new("core_runs"), Alias::new("r"))
		.join(JoinType::InnerJoin, Alias::new("authorization_execution"), Expr::col(("r", "run_id")).equals(("authorization_execution", "run_id")))
		.join(JoinType::InnerJoin, Alias::new("authorization_credentials"), Expr::col(("authorization_execution", "credential_id")).equals(("authorization_credentials", "id")))
		.and_where(Expr::col(("r", "area_id")).eq(Expr::value(area)))
		.and_where(Expr::col(("r", "tenant")).eq(Expr::value(tenant)))
		.and_where(Expr::col(("authorization_execution", "tenant")).eq(Expr::value(tenant)))
		.and_where(Expr::col(("authorization_execution", "workspace_id")).eq(Expr::value(workspace)))
		.and_where(Expr::col(("authorization_execution", "root_subject")).eq(Expr::value(owner)))
		.and_where(Expr::col(("authorization_credentials", "tenant")).eq(Expr::value(tenant)))
		.and_where(Expr::col(("authorization_credentials", "subject")).eq(Expr::value(owner)))
		.and_where(Expr::cust("authorization_credentials.revoked_at IS NULL AND authorization_credentials.expires_at > CLOCK_TIMESTAMP()"))
		.order_by(("r", "sequence"), Order::Desc).limit(1).to_string(PostgresQueryBuilder))
		.fetch_optional(&mut **lease.tx()).await?;
	let Some(grant) = grant else {
		return Ok(None);
	};
	let credential: Uuid = grant.try_get("credential_id")?;
	let subjects: Vec<String> = grant.try_get("subject_chain")?;
	if subjects.first().map(String::as_str) != Some(owner) {
		return Ok(None);
	}
	Ok(Some(
		json!({"credential":credential,"tenant":tenant,"subject":owner,"subjects":subjects}),
	))
}

async fn mark_waiting(entries: &mut Entries<'_, '_>, entry: &Entry) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("semantic_entries"))
			.value("state", "PENDING")
			.value("last_error", "awaiting_owner_run")
			.value_expr("updated_at", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("id").eq(Expr::value(entry.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	Ok(())
}

/// An owner Run can restore a missing grant without impersonating the owner.
async fn refresh_owner_authority(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	slots: &ImageSlots,
	mut authorities: Value,
) -> Result<Value> {
	let map = authorities
		.as_object_mut()
		.ok_or_else(|| Error::Conflict("invalid image authority ledger".into()))?;
	let mut entries = Entries { lease };
	for slot in &slots.entries {
		let entry_key = key(slot);
		let saved = map
			.get(&entry_key)
			.cloned()
			.ok_or_else(|| Error::Conflict("image authority unavailable".into()))?;
		if !waiting_for_owner(&saved) {
			continue;
		}
		let tenant = saved["tenant"].as_str().ok_or(Error::Forbidden)?;
		let owner = saved["subject"].as_str().ok_or(Error::Forbidden)?;
		let Some(authority) =
			owner_authority(entries.lease, workspace, slot.area_id, tenant, owner).await?
		else {
			continue;
		};
		if let Some(entry) = entries.by_key(workspace, &entry_key).await?
			&& !entry.deleted
		{
			let mut update = Query::update();
			update
				.table(Alias::new("semantic_entries"))
				.value("authority", authority.clone())
				.value_expr("updated_at", Expr::cust("CLOCK_TIMESTAMP()"))
				.and_where(Expr::col("id").eq(Expr::value(entry.id)));
			if entry.state == "PENDING" && entry.last_error.as_deref() == Some("awaiting_owner_run")
			{
				update
					.value("last_error", None::<String>)
					.value_expr("next_attempt", Expr::cust("CLOCK_TIMESTAMP()"));
			}
			native::query(&update.to_string(PostgresQueryBuilder))
				.execute(&mut **entries.lease.tx())
				.await?;
		}
		map.insert(entry_key, authority);
	}
	let waiting = !slots.legacy_candidates.is_empty() || map.values().any(waiting_for_owner);
	native::query(
		&Query::update()
			.table(Alias::new("semantic_image_ledgers"))
			.value("authorities", authorities.clone())
			.value("waiting_authority", waiting)
			.value_expr(
				"next_authority_check",
				Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '5 seconds'"),
			)
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	Ok(authorities)
}

async fn mark_limit(entries: &mut Entries<'_, '_>, workspace: Uuid, entry: &Entry) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("semantic_entries"))
			.value("state", "UNSUPPORTED")
			.value("last_error", "limit")
			.value_expr("updated_at", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("id").eq(Expr::value(entry.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	native::query(
		&Query::update()
			.table(Alias::new("semantic_points"))
			.value("retired", true)
			.value_expr("next_attempt", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("entry_id").eq(Expr::value(entry.id)))
			.and_where(Expr::col("retired").eq(Expr::value(false)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	entries
		.history(workspace, entry.id, entry.revision, "UNSUPPORTED", "limit")
		.await?;
	Ok(())
}

/// Configuration rotates active points before the quota is reconsidered.
pub(crate) async fn rebuild(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	max_images: usize,
) -> Result<()> {
	// Serialize the rebuild boundary with File publication commits.
	lock_workspace(&mut **lease.tx(), workspace).await?;
	let ledger = native::query(
		&Query::select()
			.columns(["slots", "authorities"].map(Alias::new))
			.from(Alias::new("semantic_image_ledgers"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(ledger) = ledger else {
		return Ok(());
	};
	let mut slots: ImageSlots = serde_json::from_value(ledger.try_get("slots")?)?;
	let authorities: Value = ledger.try_get("authorities")?;
	let pending: Option<i64> = native::query_scalar(
		&Query::select()
			.column("sequence")
			.from(Alias::new("semantic_image_publications"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.order_by("sequence", Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?;
	if pending.is_none() {
		slots.request_rebuild(None);
		slots.rebuild(max_images);
	} else {
		// File publication holds the same advisory lock. Record the last
		// committed event so later deletions cannot cause an implicit refill.
		slots.request_rebuild(pending);
	}
	let index = storage::index(lease.tx(), workspace, true).await?;
	let mut entries = Entries { lease };
	for slot in &slots.entries {
		let entry_key = key(slot);
		let Some(entry) = entries.by_key(workspace, &entry_key).await? else {
			continue;
		};
		if entry.deleted {
			continue;
		}
		if slot.assigned {
			let authority = authorities
				.get(entry_key.as_str())
				.ok_or_else(|| Error::Conflict("image authority unavailable".into()))?;
			if entry.state == "UNSUPPORTED" && entry.last_error.as_deref() == Some("limit") {
				let pending = entries
					.change(
						workspace,
						entry.id,
						Uuid::new_v4(),
						index.revision,
						false,
						authority.clone(),
					)
					.await?;
				entries.schedule(&pending, &index.collection).await?;
				if waiting_for_owner(authority) {
					mark_waiting(&mut entries, &pending).await?;
				}
				entries
					.history(
						workspace,
						pending.id,
						pending.revision,
						"PENDING",
						"image selected by rebuild",
					)
					.await?;
			} else if waiting_for_owner(authority) {
				mark_waiting(&mut entries, &entry).await?;
			}
			continue;
		}
		mark_limit(&mut entries, workspace, &entry).await?;
	}
	native::query(
		&Query::update()
			.table(Alias::new("semantic_image_ledgers"))
			.value("slots", serde_json::to_value(slots)?)
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	Ok(())
}

/// Seed current pre-feature images ahead of queued publications. The event
/// lock makes the snapshot and new publication order agree at commit.
async fn bootstrap(lease: &mut Lease<'_>, workspace: Uuid, slots: &ImageSlots) -> Result<bool> {
	lock_workspace(&mut **lease.tx(), workspace).await?;
	let queued = native::query(
		&Query::select()
			.columns(["area_id", "changes"].map(Alias::new))
			.from(Alias::new("semantic_image_publications"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let mut touched: HashSet<(Uuid, Uuid, String)> = slots
		.entries
		.iter()
		.map(|slot| (slot.area_id, slot.file_id, slot.path.clone()))
		.collect();
	for row in queued {
		let area: Uuid = row.try_get("area_id")?;
		let changes: Vec<aidash_domain::semantic::indexing::ImageChange> =
			serde_json::from_value(row.try_get("changes")?)?;
		touched.extend(
			changes
				.into_iter()
				.map(|change| (area, change.file_id(), change.path().to_owned())),
		);
	}
	let areas = native::query(
		&Query::select()
			.columns(["id", "tenant", "owner", "generation", "manifest"].map(Alias::new))
			.from(Alias::new("core_areas"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.order_by("id", Order::Desc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let mut seeds = Vec::new();
	for row in areas {
		let area: Uuid = row.try_get("id")?;
		let tenant: String = row.try_get("tenant")?;
		let owner: String = row.try_get("owner")?;
		let generation: i64 = row.try_get("generation")?;
		let files: Vec<aidash_domain::capabilities::operations::MountedFile> =
			serde_json::from_value(row.try_get("manifest")?)?;
		let mut changes = Vec::new();
		for (position, file) in files.iter().enumerate() {
			use aidash_domain::capabilities::operations::FileScope;
			use aidash_domain::semantic::indexing::ImageChange;
			if !matches!(file.scope, FileScope::Working | FileScope::Received)
				|| (!file.media_type.starts_with("image/")
					&& file.media_type != "application/octet-stream")
				|| touched.contains(&(area, file.file_id, file.path.clone()))
			{
				continue;
			}
			changes.push(ImageChange::LegacyCandidate {
				file_id: file.file_id,
				path: file.path.clone(),
				position,
			});
			if file.media_type.starts_with("image/") {
				changes.push(ImageChange::LegacyResolved {
					file_id: file.file_id,
					path: file.path.clone(),
					position,
					media_type: Some(file.media_type.clone()),
					size: file.size,
				});
			}
		}
		if changes.is_empty() {
			continue;
		}
		let authority = owner_authority(lease, workspace, area, &tenant, &owner)
			.await?
			.unwrap_or_else(
				|| json!({"credential":null,"tenant":tenant,"subject":owner,"subjects":[]}),
			);
		seeds.push((area, generation, changes, authority));
	}
	for (area, generation, changes, authority) in seeds {
		// SchemaExpr has no sequence operation for a negative, globally unique
		// baseline identity. The typed Query retains the sequence expression.
		let sequence = Expr::cust(
			"-nextval(pg_get_serial_sequence('public.semantic_image_publications', 'sequence')::regclass)",
		);
		native::query(
			&Query::insert()
				.into_table(Alias::new("semantic_image_publications"))
				.columns(
					[
						"sequence",
						"tenant",
						"workspace_id",
						"area_id",
						"generation",
						"changes",
						"authority",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(sequence)
						.expr(Expr::value(
							authority["tenant"].as_str().unwrap_or_default(),
						))
						.expr(Expr::value(workspace))
						.expr(Expr::value(area))
						.expr(Expr::value(generation))
						.expr(Expr::value(serde_json::to_value(changes)?))
						.expr(Expr::value(authority))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	Ok(true)
}

/// Inspect one pre-feature generic File only after restoring its owner's Run.
/// The Area lock precedes the publication lock, as in ordinary File writes.
async fn probe_legacy(store: &Store, workspace: Uuid) -> Result<()> {
	use aidash_domain::semantic::indexing::LegacyCandidate;

	let mut lookup = Lease::deployment(store, DeploymentScopeReason::SemanticMaintenance).await?;
	let row = native::query(
		&Query::select()
			.column("slots")
			.from(Alias::new("semantic_image_ledgers"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lookup.tx())
	.await?;
	let Some(row) = row else {
		return lookup.finish(Ok(())).await;
	};
	let slots: ImageSlots = serde_json::from_value(row.try_get("slots")?)?;
	let mut selected: Vec<(LegacyCandidate, Value)> = Vec::new();
	let mut authorities: HashMap<Uuid, Option<Value>> = HashMap::new();
	for candidate in &slots.legacy_candidates {
		if let Some(cached) = authorities.get(&candidate.area_id) {
			if let Some(authority) = cached {
				selected.push((candidate.clone(), authority.clone()));
			}
			continue;
		}
		let area = native::query(
			&Query::select()
				.columns(["tenant", "owner"].map(Alias::new))
				.from(Alias::new("core_areas"))
				.and_where(Expr::col("id").eq(Expr::value(candidate.area_id)))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lookup.tx())
		.await?;
		let Some(area) = area else {
			authorities.insert(candidate.area_id, None);
			continue;
		};
		let tenant: String = area.try_get("tenant")?;
		let owner: String = area.try_get("owner")?;
		let authority =
			owner_authority(&mut lookup, workspace, candidate.area_id, &tenant, &owner).await?;
		if let Some(authority) = &authority {
			selected.push((candidate.clone(), authority.clone()));
		}
		authorities.insert(candidate.area_id, authority);
	}
	lookup.finish(Ok(())).await?;
	let mut failed_areas = BTreeSet::new();
	let mut processed = 0;
	for (candidate, authority) in selected {
		if failed_areas.contains(&candidate.area_id) {
			continue;
		}
		let area_id = candidate.area_id;
		match probe_one_legacy(store, workspace, candidate, authority).await {
			Ok(()) => {
				processed += 1;
				if processed == 8 {
					break;
				}
			}
			Err(Error::Forbidden | Error::Unauthorized) => {
				failed_areas.insert(area_id);
				tracing::warn!("legacy image authority or File unavailable");
			}
			Err(_) => tracing::warn!("legacy File signature probe failed"),
		}
	}
	Ok(())
}

async fn probe_one_legacy(
	store: &Store,
	workspace: Uuid,
	candidate: aidash_domain::semantic::indexing::LegacyCandidate,
	authority: Value,
) -> Result<()> {
	use crate::apps::execution::capabilities::{
		serializers::contracts::{FileEntry, FileScope},
		services::sessions,
	};
	use aidash_domain::semantic::indexing::ImageChange;
	use tokio::io::AsyncReadExt as _;
	let mut lease = Lease::restore(store, authority.clone(), workspace).await?;
	let result: Result<()> = async {
		let access = lease.access().ok_or(Error::Forbidden)?;
		let mut area = match sessions::load(access, candidate.area_id).await {
			Err(Error::NotFound(_)) => return Err(Error::Forbidden),
			other => other?,
		};
		if area.workspace_id != workspace { return Err(Error::Forbidden); }
		// The candidate may have been removed while its authority was restored.
		let ledger = native::query(&Query::select().column("slots").from(Alias::new("semantic_image_ledgers"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace))).to_string(PostgresQueryBuilder))
			.fetch_optional(&mut **access.tx).await?;
		let Some(ledger) = ledger else { return Ok(()); };
		let current: ImageSlots = serde_json::from_value(ledger.try_get("slots")?)?;
		if !current.legacy_candidates.contains(&candidate) { return Ok(()); }
		let mut files: Vec<FileEntry> = serde_json::from_value(area.manifest.clone())?;
		let mut media_type = None;
		let mut size = 0;
		let mut changed = false;
		if area.generation == candidate.generation
			&& let Some(file) = files.iter_mut().find(|file| file.file_id == candidate.file_id && file.path == candidate.path && matches!(file.scope, FileScope::Working | FileScope::Received)) {
				size = file.size;
				if file.media_type == "application/octet-stream" {
					let verified = store.capabilities.verified(access, file).await?;
					let mut prefix = Vec::new();
					verified.take(12).read_to_end(&mut prefix).await?;
					if let Some(detected) = aidash_domain::provider::detect_image(&prefix) {
						file.media_type = detected.into();
						media_type = Some(detected.to_owned());
						changed = true;
					}
				} else if file.media_type.starts_with("image/") {
					media_type = Some(file.media_type.clone());
				}
		}
		lock_workspace(&mut **access.tx, workspace).await?;
		if changed {
			area.revision += 1;
			area.manifest = serde_json::to_value(files)?;
			native::query(&Query::update().table(Alias::new("core_areas"))
				.value("manifest", area.manifest.clone()).value("revision", area.revision)
				.and_where(Expr::col("id").eq(Expr::value(area.id))).to_string(PostgresQueryBuilder))
				.execute(&mut **access.tx).await?;
			store.event(&mut access.tx, Some(workspace), "capability.files_changed", json!({"area_id":area.id,"revision":area.revision,"generation":area.generation})).await?;
		}
		let change = ImageChange::LegacyResolved { file_id: candidate.file_id, path: candidate.path.clone(), position: candidate.position, media_type, size };
		native::query(&Query::insert().into_table(Alias::new("semantic_image_publications"))
			.columns(["sequence", "tenant", "workspace_id", "area_id", "generation", "changes", "authority"].map(Alias::new))
			.from_subquery(Query::select().expr(Expr::cust("-nextval(pg_get_serial_sequence('public.semantic_image_publications', 'sequence')::regclass)"))
				.expr(Expr::value(area.tenant)).expr(Expr::value(workspace)).expr(Expr::value(area.id)).expr(Expr::value(candidate.generation))
				.expr(Expr::value(serde_json::to_value(vec![change])?)).expr(Expr::value(authority)).to_owned())
			.to_string(PostgresQueryBuilder)).execute(&mut **access.tx).await?;
		Ok(())
	}.await;
	lease.finish(result).await
}

/// Bounded discovery skips Workspaces that have not configured an index yet.
pub(crate) async fn sweep(store: &Store) -> Result<()> {
	let pending_baselines = native::query(
		&Query::select()
			.column(("semantic_indexes", "workspace_id"))
			.from(Alias::new("semantic_indexes"))
			.join(
				JoinType::LeftJoin,
				Alias::new("semantic_image_ledgers"),
				Expr::col(("semantic_indexes", "workspace_id"))
					.equals(("semantic_image_ledgers", "workspace_id")),
			)
			.and_where(Expr::cust("semantic_indexes.spec->>'enabled' = 'true'"))
			.and_where(Expr::cust(
				"semantic_image_ledgers.workspace_id IS NULL OR NOT semantic_image_ledgers.initialized OR (semantic_image_ledgers.waiting_authority AND semantic_image_ledgers.next_authority_check <= CLOCK_TIMESTAMP())",
			))
			.order_by_expr(
				Expr::cust("COALESCE(semantic_image_ledgers.next_authority_check, '-infinity'::timestamptz)"),
				Order::Asc,
			)
			.order_by(("semantic_indexes", "workspace_id"), Order::Asc)
			.limit(64)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.in_scope(&MAINTENANCE))
	.await?;
	let rows = native::query(
		&Query::select()
			.column(("p", "workspace_id"))
			.expr_as(
				Func::min(Expr::col(("p", "sequence")).into()),
				Alias::new("oldest"),
			)
			.from_as(Alias::new("semantic_image_publications"), Alias::new("p"))
			.join(
				JoinType::InnerJoin,
				Alias::new("semantic_indexes"),
				Expr::col(("p", "workspace_id")).equals(("semantic_indexes", "workspace_id")),
			)
			.and_where(Expr::cust("semantic_indexes.spec->>'enabled' = 'true'"))
			.group_by_col(("p", "workspace_id"))
			.order_by(Alias::new("oldest"), Order::Asc)
			.limit(64)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.in_scope(&MAINTENANCE))
	.await?;
	let mut seen = BTreeSet::new();
	for row in pending_baselines.into_iter().chain(rows) {
		let workspace: Uuid = row.try_get("workspace_id")?;
		if !seen.insert(workspace) {
			continue;
		}
		if probe_legacy(store, workspace).await.is_err() {
			tracing::warn!("legacy image probe deferred");
		}
		for _ in 0..8 {
			let lease =
				Lease::deployment(store, DeploymentScopeReason::SemanticMaintenance).await?;
			let session = Session {
				lease: Some(lease),
				workspace,
				index: None,
				authorities: json!({}),
			};
			match aidash_application::semantic::image_publications::process(Box::new(session)).await
			{
				Ok(true) => continue,
				Ok(false) => break,
				Err(_) => {
					// One Area's stale authority or persistence error must not block text
					// indexing or other Workspaces; its durable event remains for retry.
					tracing::warn!("image publication deferred");
					break;
				}
			}
		}
	}
	Ok(())
}

#[async_trait]
impl ImagePublicationSession for Session {
	async fn load(&mut self) -> aidash_application::Result<Option<ImagePublication>> {
		let workspace = self.workspace;
		let lease = self.lease()?;
		crate::apps::workspaces::models::Workspace::lock_existing(lease.tx().as_mut(), workspace)
			.await?;
		let index = storage::index(lease.tx(), workspace, true).await?;
		let spec: IndexingSpec = serde_json::from_value(index.spec.clone())?;
		if !spec.enabled {
			return Ok(None);
		}
		let ledger = native::query(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("semantic_image_ledgers"))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		let (slots, mut authorities, initialized, waiting_authority) = if let Some(ledger) = ledger
		{
			(
				serde_json::from_value(ledger.try_get("slots")?)?,
				ledger.try_get("authorities")?,
				ledger.try_get::<bool>("initialized")?,
				ledger.try_get::<bool>("waiting_authority")?,
			)
		} else {
			native::query(
				&Query::insert()
					.into_table(Alias::new("semantic_image_ledgers"))
					.columns(
						[
							"workspace_id",
							"tenant",
							"slots",
							"authorities",
							"initialized",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::value(workspace))
							.expr(Expr::value(index.tenant.clone()))
							.expr(Expr::value(serde_json::to_value(ImageSlots::default())?))
							.expr(Expr::value(json!({})))
							.expr(Expr::value(false))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::column(Alias::new("workspace_id"))
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			(ImageSlots::default(), json!({}), false, false)
		};
		if waiting_authority {
			authorities = refresh_owner_authority(lease, workspace, &slots, authorities).await?;
		}
		if !initialized {
			if !bootstrap(lease, workspace, &slots).await? {
				return Ok(None);
			}
			native::query(
				&Query::update()
					.table(Alias::new("semantic_image_ledgers"))
					.value("initialized", true)
					.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
		}
		let row = native::query(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("semantic_image_publications"))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.order_by("sequence", Order::Asc)
				.lock(LockType::Update)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		let Some(row) = row else {
			return Ok(None);
		};
		let sequence: i64 = row.try_get("sequence")?;
		let rebuild_after = slots.rebuild_through == Some(sequence);
		let publication = ImagePublication {
			workspace,
			area: row.try_get("area_id")?,
			generation: row.try_get("generation")?,
			sequence,
			changes: serde_json::from_value(row.try_get("changes")?)?,
			authority: row.try_get("authority")?,
			max_images: spec.max_images,
			slots,
			rebuild_after,
		};
		self.index = Some(index);
		self.authorities = authorities;
		Ok(Some(publication))
	}

	async fn store(
		&mut self,
		publication: &ImagePublication,
		before: &ImageSlots,
		after: &ImageSlots,
	) -> aidash_application::Result<()> {
		let index = self
			.index
			.clone()
			.ok_or_else(|| Error::Conflict("image index unavailable".into()))?;
		let mut authorities = self.authorities.clone();
		let map = authorities
			.as_object_mut()
			.ok_or_else(|| Error::Conflict("invalid image authority ledger".into()))?;
		let lease = self.lease()?;
		let mut entries = Entries { lease };
		for old in &before.entries {
			if after
				.entries
				.iter()
				.any(|new| identity(new) == identity(old))
			{
				continue;
			}
			let entry_key = key(old);
			if let Some(entry) = entries.by_key(publication.workspace, &entry_key).await?
				&& !entry.deleted
			{
				let deleted = entries
					.change(
						publication.workspace,
						entry.id,
						Uuid::new_v4(),
						index.revision,
						true,
						publication.authority.clone(),
					)
					.await?;
				entries.schedule(&deleted, &index.collection).await?;
				entries
					.history(
						publication.workspace,
						deleted.id,
						deleted.revision,
						"DELETED",
						"File version replaced or removed",
					)
					.await?;
			}
			map.remove(&entry_key);
		}
		for slot in &after.entries {
			if let Some(old) = before
				.entries
				.iter()
				.find(|old| identity(old) == identity(slot))
			{
				if old.assigned != slot.assigned {
					let entry_key = key(slot);
					if let Some(entry) = entries.by_key(publication.workspace, &entry_key).await?
						&& !entry.deleted
					{
						if slot.assigned {
							let authority = map.get(&entry_key).cloned().ok_or_else(|| {
								Error::Conflict("image authority unavailable".into())
							})?;
							let waiting = waiting_for_owner(&authority);
							let pending = entries
								.change(
									publication.workspace,
									entry.id,
									Uuid::new_v4(),
									index.revision,
									false,
									authority,
								)
								.await?;
							entries.schedule(&pending, &index.collection).await?;
							if waiting {
								mark_waiting(&mut entries, &pending).await?;
							}
							entries
								.history(
									publication.workspace,
									pending.id,
									pending.revision,
									"PENDING",
									"image selected by rebuild",
								)
								.await?;
						} else {
							mark_limit(&mut entries, publication.workspace, &entry).await?;
						}
					}
				}
				continue;
			}
			let entry_key = key(slot);
			let source = Source::WorkingImage {
				area_id: slot.area_id,
				generation: slot.generation,
				file_id: slot.file_id,
				path: slot.path.clone(),
			};
			let entry = Entry {
				id: Uuid::new_v4(),
				workspace_id: publication.workspace,
				key: entry_key.clone(),
				source: serde_json::to_value(source)?,
				agent: None,
				metadata: json!({"name":slot.path.rsplit('/').next().unwrap_or(&slot.path),"media_type":slot.media_type,"size":slot.size}),
				revision: 1,
				point_id: Uuid::new_v4(),
				index_revision: index.revision,
				deleted: false,
				state: "PENDING".into(),
				attempts: 0,
				last_error: None,
				created_by: publication.authority["subject"]
					.as_str()
					.unwrap_or_default()
					.into(),
				updated_at: Utc::now(),
			};
			let entry = entries.put(entry, publication.authority.clone()).await?;
			entries.schedule(&entry, &index.collection).await?;
			if !slot.assigned {
				mark_limit(&mut entries, publication.workspace, &entry).await?;
			} else {
				if waiting_for_owner(&publication.authority) {
					mark_waiting(&mut entries, &entry).await?;
				}
				entries
					.history(
						publication.workspace,
						entry.id,
						entry.revision,
						"PENDING",
						"File version published",
					)
					.await?;
			}
			map.insert(entry_key, publication.authority.clone());
		}
		let waiting = !after.legacy_candidates.is_empty() || map.values().any(waiting_for_owner);
		let lease = self.lease()?;
		let mut update = Query::update();
		update
			.table(Alias::new("semantic_image_ledgers"))
			.value("slots", serde_json::to_value(after)?)
			.value("authorities", authorities)
			.value("waiting_authority", waiting)
			.value_expr("next_authority_check", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(publication.workspace)));
		if publication.rebuild_after {
			update.value("initialized", true);
		}
		native::query(&update.to_string(PostgresQueryBuilder))
			.execute(&mut **lease.tx())
			.await?;
		native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_image_publications"))
				.and_where(Expr::col("sequence").eq(Expr::value(publication.sequence)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		Ok(())
	}

	async fn commit(mut self: Box<Self>) -> aidash_application::Result<()> {
		let lease = self
			.lease
			.take()
			.ok_or_else(|| Error::Conflict("image publication transaction ended".into()))?;
		lease.finish(Ok(())).await.map_err(Into::into)
	}
}

#[cfg(test)]
mod tests {
	use super::lock_workspace;
	use crate::apps::execution::test_database::{DatabaseFixture, database};
	use rstest::rstest;
	use std::time::Duration;
	use uuid::Uuid;

	#[rstest]
	#[tokio::test]
	async fn image_publication_lock_serializes_only_the_same_workspace(
		#[future] database: DatabaseFixture,
	) {
		let database = database.await;
		let workspace = Uuid::new_v4();
		let mut first = database.begin_fixture().await;
		let mut same = database.begin_fixture().await;
		let mut other = database.begin_fixture().await;
		lock_workspace(&mut *first, workspace).await.unwrap();
		assert!(
			tokio::time::timeout(
				Duration::from_secs(2),
				lock_workspace(&mut *other, Uuid::new_v4())
			)
			.await
			.unwrap()
			.is_ok()
		);
		assert!(
			tokio::time::timeout(
				Duration::from_millis(100),
				lock_workspace(&mut *same, workspace)
			)
			.await
			.is_err()
		);
		first.commit().await.unwrap();
		assert!(
			tokio::time::timeout(
				Duration::from_secs(2),
				lock_workspace(&mut *same, workspace)
			)
			.await
			.unwrap()
			.is_ok()
		);
	}
}

//! Memory-only salvage restores retain the live authority, origin budgets and primary journal.
//! Closing the external gate precedes PostgreSQL locks. Every failure leaves it closed.
use super::native_memory;
use crate::apps::knowledge::repositories::{
	access::Lease,
	bank_settings, native_memory as repository, purge,
	recovery::{self, FileRecovery},
	unit_origins, units,
};
use crate::{Error, Result, database::native, store::Store};
use aidash_application::ports::{VectorIndex, memory::MemoryRecovery};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::memory::{Bank, Policy, Unit};
use aidash_domain::registry::EntityRef;
use chrono::{DateTime, Duration, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use std::{
	path::{Path, PathBuf},
	sync::Arc,
};
use uuid::Uuid;

const RECOVERY: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::MemoryRecovery);

pub mod protection;

const MAX_ARCHIVES: usize = 128;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
	format: u32,
	home: String,
	epoch: Uuid,
	bank: Bank,
	provider: EntityRef,
	settings_revision: i64,
	created_at: DateTime<Utc>,
	expires_at: DateTime<Utc>,
	units: Vec<Unit>,
}
#[derive(Debug, Serialize)]
pub struct Report {
	pub restored: usize,
	pub withheld: usize,
	pub epoch: Uuid,
}

/// Attach an existing external ledger, including its persistent closed state.
/// A missing/corrupt ledger is never initialized implicitly.
pub async fn attach(store: Store, directory: PathBuf) -> Result<Store> {
	let ledger = external(&store, &directory)?;
	attach_ledger(store, ledger).await
}
/// Compose an independently durable store, also used by supervised recovery tools.
pub async fn attach_protected(
	store: Store,
	directory: PathBuf,
	objects: Arc<dyn aidash_application::ports::memory::objects::RecoveryObjects>,
) -> Result<Store> {
	let ledger = FileRecovery::new(directory, store.deployment.to_string())?.with_objects(objects);
	attach_ledger(store, ledger).await
}
async fn attach_ledger(store: Store, ledger: FileRecovery) -> Result<Store> {
	// Missing/corrupt protection closes only memory, preserving management access.
	let _ = ledger.verify().await;
	Ok(store.with_memory_recovery(Some(Arc::new(ledger))))
}
fn external(store: &Store, directory: &Path) -> Result<FileRecovery> {
	let mut recovery = FileRecovery::from_env(directory.to_owned(), store.deployment.to_string())?;
	if let Some(objects) = store
		.pool
		.memory_recovery()
		.and_then(|active| active.objects())
	{
		recovery = recovery.with_objects(objects);
	}
	Ok(recovery)
}
async fn active_external(store: &Store, directory: &Path) -> Result<FileRecovery> {
	let recovery = external(store, directory)?;
	let active = store
		.pool
		.memory_recovery()
		.ok_or(Error::SemanticUnavailable)?;
	if active.epoch()? != recovery.load()?.epoch {
		return Err(Error::Forbidden);
	}
	recovery.verify().await?;
	Ok(recovery)
}
/// Activate independent protection after every floor-advancing writer is fenced.
/// This seeds every local shard, including floors ahead of PostgreSQL.
pub async fn protect(store: &Store, directory: &Path) -> Result<Store> {
	let recovery = external(store, directory)?;
	recovery.protect().await?;
	attach_ledger(store.clone(), recovery).await
}
/// Rebuild a replacement disk behind a new create-only recovery barrier.
pub async fn reconstruct(
	store: &Store,
	directory: &Path,
) -> Result<aidash_domain::memory::recovery::history::Head> {
	external(store, directory)?.reconstruct().await
}
fn bypass(store: &Store) -> Store {
	let mut internal = store.clone();
	internal.pool = internal.pool.memory_recovery_internal();
	internal.control_pool = internal.control_pool.memory_recovery_internal();
	internal
}

/// The address of the Node a Workspace belongs to, which is the home of its banks.
async fn workspace_home(store: &Store, workspace: Uuid) -> Result<String> {
	let internal = bypass(store);
	let mut lease = Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
	let result = async {
		let node: Option<String> = native::query_scalar(
			&Query::select()
				.column(Alias::new("node"))
				.from(Alias::new("workspaces"))
				.and_where(Expr::col("id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **lease.tx())
		.await?;
		let node = node.ok_or_else(|| Error::NotFound("workspace".into()))?;
		Ok(format!("{}/{node}", store.deployment))
	}
	.await;
	lease.finish(result).await
}

/// Explicit first initialization of the new relational format, never a JSON import.
pub async fn initialize(store: &Store, directory: PathBuf) -> Result<Store> {
	let recovery = external(store, &directory)?;
	let internal = bypass(store);
	let mut lease = Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
	let result = async {
		for table in ["memory_banks","memory_participants"] {
			if native::query(&Query::select().column(Alias::new("id")).from(Alias::new(table))
				.and_where(crate::database::scope::in_deployment("home", store.deployment.as_str())).limit(1).to_string(PostgresQueryBuilder))
				.fetch_optional(&mut **lease.tx()).await?.is_some() {
				return Err(Error::Conflict("a new recovery epoch requires empty new-format memory; restore the existing external ledger".into()));
			}
		}
		recovery.initialize(&[]).await
	}
	.await;
	lease.finish(result).await?;
	attach(store.clone(), directory).await
}

/// Explicit deployment bootstrap preserves every existing epoch and closed gate.
/// Partial or corrupt state requires recovery; it is never replaced.
pub async fn initialize_if_missing(store: &Store, directory: PathBuf) -> Result<Store> {
	let recovery = external(store, &directory)?;
	if directory.join("epoch.cbor").exists() || directory.join("ledger.cbor").exists() {
		recovery.load()?;
		recovery.verify().await?;
		attach(store.clone(), directory).await
	} else {
		initialize(store, directory).await
	}
}

async fn selected(lease: &mut Lease<'_>, bank: &Bank, limit: usize) -> Result<Vec<Unit>> {
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let ids: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(limit as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await?;
	if ids.len() > limit {
		return Err(Error::Invalid(
			"memory recovery exceeds its bank record cap".into(),
		));
	}
	let mut result = Vec::with_capacity(ids.len());
	for id in ids {
		result.push(
			units::load(lease, id, false)
				.await?
				.ok_or(Error::Forbidden)?,
		);
	}
	Ok(result)
}
async fn configuration(
	lease: &mut Lease<'_>,
	bank: &Bank,
) -> Result<(bank_settings::Settings, Policy)> {
	units::authorize(lease, bank, "memory.configure").await?;
	let settings = bank_settings::get(lease, bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = native_memory::policy(lease, &bank.tenant, &settings.provider).await?;
	Ok((settings, policy))
}

/// Backups live in the external managed directory and expire under the pinned bank policy.
pub async fn backup(store: &Store, directory: &Path, bank: Bank) -> Result<PathBuf> {
	backup_to(store, directory, bank, None).await
}
async fn backup_to(
	store: &Store,
	directory: &Path,
	bank: Bank,
	spool: Option<protection::SpoolLimits>,
) -> Result<PathBuf> {
	let recovery = active_external(store, directory).await?;
	recovery.require_serving()?;
	let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	let result = async {
		repository::lock_workspace(&mut lease, bank.workspace, false).await?;
		let (settings, policy) = configuration(&mut lease, &bank).await?;
		let ledger = recovery.load()?;
		let contents = selected(&mut lease, &bank, policy.retention.max_unit_records).await?;
		for unit in &contents {
			let fence = ledger.units.get(&unit.id).ok_or(Error::Forbidden)?;
			if fence.bank != unit.bank
				|| fence.revision != unit.revision
				|| fence.digest != aidash_domain::memory::recovery::digest(unit)?
			{
				return Err(Error::Conflict(
					"memory recovery ledger and database differ".into(),
				));
			}
			if unit.visible() {
				validate_writer(store, unit, &policy).await?;
			}
		}
		let now = Utc::now();
		let archive = Archive {
			format: 1,
			home: store.deployment.to_string(),
			epoch: ledger.epoch,
			bank,
			provider: settings.provider,
			settings_revision: settings.revision,
			created_at: now,
			expires_at: now + Duration::days(i64::from(policy.retention.backup_days)),
			units: contents,
		};
		let archives = directory.join(if spool.is_some() { "spool" } else { "archives" });
		std::fs::create_dir_all(&archives)?;
		if spool.is_none()
			&& prune(
				&mut lease,
				&archives,
				store.deployment.as_str(),
				ledger.epoch,
				now,
			)
			.await? >= MAX_ARCHIVES
		{
			return Err(Error::Conflict(
				"managed memory backup capacity reached".into(),
			));
		}
		if let Some(limits) = spool {
			limits.require_capacity(&archives, &archive)?;
		}
		let path = archives.join(format!("{}.cbor", Uuid::new_v4()));
		recovery::atomic_write(&path, &archive)?;
		Ok(path)
	}
	.await;
	lease.finish(result).await
}

async fn prune(
	lease: &mut Lease<'_>,
	directory: &Path,
	home: &str,
	epoch: Uuid,
	now: DateTime<Utc>,
) -> Result<usize> {
	let mut count = 0;
	// Revisions pin restore eligibility. Retired archives must not occupy the
	// global capacity after a policy upgrade, even with long original retention.
	let mut policies = Vec::<(Bank, Option<(bank_settings::Settings, u32)>)>::new();
	for entry in std::fs::read_dir(directory)? {
		let path = entry?.path();
		if path.extension().is_none_or(|extension| extension != "cbor") {
			continue;
		}
		let archive: Archive = recovery::read(&path)?;
		let settings =
			if let Some((_, settings)) = policies.iter().find(|(bank, _)| bank == &archive.bank) {
				settings.clone()
			} else {
				let settings = match bank_settings::get(lease, &archive.bank).await? {
					Some(settings) => {
						let policy =
							native_memory::policy(lease, &archive.bank.tenant, &settings.provider)
								.await?;
						Some((settings, policy.retention.backup_days))
					}
					None => None,
				};
				policies.push((archive.bank.clone(), settings.clone()));
				settings
			};
		let eligible = settings.is_some_and(|(settings, days)| {
			settings.revision == archive.settings_revision
				&& settings.provider == archive.provider
				&& archive.created_at + Duration::days(i64::from(days)) > now
		});
		if !eligible
			|| archive.format != 1
			|| archive.home != home
			|| archive.epoch != epoch
			|| archive.expires_at <= now
		{
			std::fs::remove_file(path)?;
		} else {
			count += 1;
		}
	}
	std::fs::File::open(directory)?.sync_all()?;
	Ok(count)
}

fn clear(unit: &mut Unit) {
	unit.content.text.clear();
	unit.content.mental_model = None;
	unit.content.entities.clear();
	// Exact evidence identities are retained for transitive purge/negative
	// lineage. They contain no quoted source body and never authorize stale use.
	unit.content.links.clear();
	unit.content.occurred = None;
}

/// Restore a bank into a live Home database. Authority, primary evidence, origin
/// metadata, budget reservations and request/deletion receipts are never rewound.
pub async fn restore(store: &Store, directory: &Path, path: &Path) -> Result<Report> {
	let recovery = active_external(store, directory).await?;
	let archive: Archive = recovery::read(path)?;
	let ledger = recovery.load()?;
	if archive.format != 1
		|| archive.home != store.deployment
		|| aidash_domain::nodes::deployment_of(&archive.bank.home) != store.deployment
		|| archive.epoch != ledger.epoch
		|| archive.created_at > Utc::now()
		|| archive.expires_at <= Utc::now()
		|| archive.created_at >= archive.expires_at
	{
		return Err(Error::Forbidden);
	}
	// Release the filesystem lock before waiting for PostgreSQL locks. In-flight
	// writers then observe this closed gate and roll back without lock inversion.
	let ledger = recovery.gate(true, ledger.epoch)?;
	let internal = bypass(store);
	let mut lease = Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
	let result = async {
		repository::lock_workspace(&mut lease, archive.bank.workspace, true).await?;
		let (settings, policy) = configuration(&mut lease, &archive.bank).await?;
		if settings.revision != archive.settings_revision
			|| settings.provider != archive.provider
			|| archive.units.len() > policy.retention.max_unit_records
			|| archive.created_at + Duration::days(i64::from(policy.retention.backup_days))
				<= Utc::now()
		{
			return Err(Error::Conflict(
				"live memory policy differs from the archive".into(),
			));
		}
		let bank_id = repository::bank_id(&mut lease, &archive.bank, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let mut seen = std::collections::BTreeSet::new();
		for old in &archive.units {
			if old.bank != archive.bank || !seen.insert(old.id) {
				return Err(Error::Forbidden);
			}
			let floor = ledger.units.get(&old.id).ok_or(Error::Forbidden)?;
			if floor.bank != archive.bank {
				return Err(Error::Forbidden);
			}
			let current = units::load(&mut lease, old.id, true).await?;
			if current
				.as_ref()
				.is_some_and(|value| ledger.current(value).unwrap_or(false))
			{
				continue;
			}
			let mut unit = old.clone();
			if !ledger.current(old)? {
				unit.revision = if floor.deleted {
					// A deletion receipt and its existing purge job identify the
					// same revision. Restore must preserve that negative identity.
					floor.revision
				} else {
					floor
						.revision
						.checked_add(1)
						.filter(|revision| *revision < i64::MAX)
						.ok_or(Error::Forbidden)?
				};
				unit.deleted = floor.deleted;
				unit.stale = !floor.deleted;
				clear(&mut unit);
			}
			let origin = unit_origins::load(&mut lease, unit.id)
				.await?
				.ok_or(Error::Forbidden)?;
			if origin.revision != floor.revision {
				unit.deleted = floor.deleted;
				unit.stale = !floor.deleted;
				clear(&mut unit);
			}
			if !unit.visible() {
				recovery.advance_restore(ledger.epoch, &unit).await?;
			}
			repository::restore_body(&mut lease, bank_id, &unit).await?;
			if !unit.visible() {
				unit_origins::record(&mut lease, &unit, None, None).await?;
				if unit.deleted {
					purge::schedule(&mut lease, bank_id, &unit, &policy.retention).await?;
				}
			}
		}
		Ok(policy)
	}
	.await;
	let policy = lease.finish(result).await?;
	finish(
		&internal,
		&recovery,
		&archive.bank,
		&archive.provider,
		ledger.epoch,
		&policy,
	)
	.await
}

async fn validate_writer(store: &Store, unit: &Unit, policy: &Policy) -> Result<()> {
	let mut lookup = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	let origin = unit_origins::load(&mut lookup, unit.id)
		.await?
		.ok_or(Error::Forbidden)?;
	lookup.finish(Ok(())).await?;
	if origin.revision != unit.revision {
		return Err(Error::Forbidden);
	}
	for id in origin.runs {
		let record: aidash_domain::Run = crate::database::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(store.pool.in_scope(&RECOVERY))
		.await?;
		if record.home_node != unit.bank.home || record.workspace_id != unit.bank.workspace {
			return Err(Error::Forbidden);
		}
		let mut access = crate::authorization::execution::proof_for_run(store, &record.metadata())
			.await?
			.ok_or(Error::Forbidden)?;
		// Exact provider roles and generated origin allowance are rechecked
		// before any rebuild can invoke a model. No reservation is refunded.
		for (reference, kind) in [
			(&policy.extraction, "model"),
			(&policy.derivation, "model"),
			(&policy.reflection, "model"),
			(&policy.embedding, "embedding"),
			(&policy.reranker, "reranker"),
			(&policy.tokenizer, "tokenizer"),
		] {
			native_memory::definition(
				&mut Lease::Inherited(&mut access),
				&unit.bank.tenant,
				reference,
				kind,
			)
			.await?;
		}
		access.finish(Ok(())).await?;
	}
	let mut writer = Lease::restore_proof(
		store,
		serde_json::to_value(origin.authority)?,
		unit.bank.workspace,
	)
	.await?;
	let result = async {
		units::authorize(&mut writer, &unit.bank, "memory.read").await?;
		units::current(
			&mut writer,
			unit.bank.workspace,
			&unit.content.evidence,
			policy.bounds.max_graph_visits,
		)
		.await
	}
	.await;
	writer.finish(result).await
}

async fn finish(
	store: &Store,
	recovery: &FileRecovery,
	bank: &Bank,
	provider: &EntityRef,
	epoch: Uuid,
	policy: &Policy,
) -> Result<Report> {
	let mut report = Report {
		restored: 0,
		withheld: 0,
		epoch,
	};
	// Invalid support is cleared before each next pass. A dependent visited
	// earlier must be reconsidered after its source was withheld later.
	for pass in 0..=policy.bounds.max_graph_visits {
		let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
		let contents = selected(&mut lease, bank, policy.retention.max_unit_records).await?;
		lease.finish(Ok(())).await?;
		let mut ledger = recovery.load()?;
		let mut changed = false;
		for mut unit in contents.into_iter().filter(Unit::visible) {
			let ttl = policy.retention.unit_expired(unit.learned_at, Utc::now());
			// The pass snapshot is already validated. Check only this fence so
			// Ledger validation does not rescan every Home identity per unit.
			let mut selected = aidash_domain::memory::recovery::Ledger::new(
				aidash_domain::nodes::deployment_of(&bank.home).to_owned(),
			);
			if let Some(fence) = ledger.units.get(&unit.id) {
				selected.units.insert(unit.id, fence.clone());
			}
			let fenced = selected.current(&unit)?;
			if !ttl && fenced {
				match validate_writer(store, &unit, policy).await {
					Ok(()) => continue,
					Err(
						Error::Forbidden
						| Error::Unauthorized
						| Error::Conflict(_)
						| Error::Invalid(_),
					) => {}
					Err(error) => return Err(error),
				}
			}
			let floor = ledger
				.units
				.get(&unit.id)
				.ok_or(Error::SemanticUnavailable)?;
			// A failed SQL deletion still published an irrevocable negative receipt.
			// Reconcile that exact identity instead of inventing another deletion revision.
			unit.revision = if floor.deleted {
				floor.revision
			} else {
				floor
					.revision
					.max(unit.revision)
					.checked_add(1)
					.filter(|revision| *revision < i64::MAX)
					.ok_or(Error::Forbidden)?
			};
			unit.deleted = ttl || floor.deleted;
			unit.stale = !unit.deleted;
			unit.updated_at = Utc::now();
			clear(&mut unit);
			recovery.advance_restore(epoch, &unit).await?;
			ledger.observe(&unit)?;
			let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
			repository::lock_workspace(&mut lease, bank.workspace, true).await?;
			let bank_id = repository::bank_id(&mut lease, bank, false)
				.await?
				.ok_or(Error::Forbidden)?;
			repository::restore_body(&mut lease, bank_id, &unit).await?;
			unit_origins::record(&mut lease, &unit, None, None).await?;
			if unit.deleted {
				purge::schedule(&mut lease, bank_id, &unit, &policy.retention).await?;
			}
			lease.finish(Ok(())).await?;
			report.withheld += 1;
			changed = true;
		}
		if !changed {
			break;
		}
		if pass == policy.bounds.max_graph_visits {
			return Err(Error::Conflict(
				"restore invalidation exceeds its declared traversal bound".into(),
			));
		}
	}
	let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	repository::lock_workspace(&mut lease, bank.workspace, true).await?;
	let mut contents = selected(&mut lease, bank, policy.retention.max_unit_records).await?;
	let bank_id = repository::bank_id(&mut lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let floors = recovery.load()?;
	for unit in &mut contents {
		if !unit.visible() {
			let floor = floors
				.units
				.get(&unit.id)
				.ok_or(Error::SemanticUnavailable)?;
			// Hidden SQL rows can also lag a deletion whose COMMIT failed.
			// Preserve the permanent receipt before clearing their remaining body.
			unit.revision = if floor.deleted {
				floor.revision
			} else {
				floor.revision.max(unit.revision)
			};
			unit.deleted |= floor.deleted;
			// A current stale unit may still carry a body pending ordinary
			// cleanup. The recovery gate opens only after removing that body.
			clear(unit);
			recovery.advance_restore(epoch, unit).await?;
			repository::restore_body(&mut lease, bank_id, unit).await?;
			unit_origins::record(&mut lease, unit, None, None).await?;
			if unit.deleted {
				purge::schedule(&mut lease, bank_id, unit, &policy.retention).await?;
			}
			let points = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("semantic_points"))
				.and_where(Expr::col("entry_id").eq(Expr::value(unit.id)))
				.to_owned();
			native::query(
				&Query::delete()
					.from_table(Alias::new("semantic_vectors"))
					.and_where(Expr::col("id").in_subquery(points))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
		}
		repository::refresh_dependencies(&mut lease, bank_id, unit).await?;
		// Older result/history/candidate bodies are disposable. They cannot be
		// included in a restore or outlive its current authority validation.
		native::query(
			&Query::delete()
				.from_table(Alias::new("memory_history"))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		let row = native::query(
			&Query::select()
				.columns(["point_id", "state", "metadata", "index_revision"].map(Alias::new))
				.from(Alias::new("semantic_entries"))
				.and_where(Expr::col("id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		let mut reusable = false;
		if unit.visible()
			&& let Some(row) = row
		{
			let metadata: serde_json::Value = row.try_get("metadata")?;
			let state: String = row.try_get("state")?;
			if let Some(index) = crate::semantic::models::SemanticIndexe::locked(
				&mut **lease.tx(),
				bank.workspace,
				false,
			)
			.await?
			{
				reusable = matches!(state.as_str(), "PENDING" | "READY")
					&& metadata["unit_revision"].as_i64() == Some(unit.revision)
					&& metadata["provider"] == serde_json::to_value(provider)?
					&& row.try_get::<i64>("index_revision")? == index.revision;
				if reusable && state == "READY" {
					let digest: Option<String> = native::query_scalar(
						&Query::select()
							.column(Alias::new("content_digest"))
							.from(Alias::new("semantic_points"))
							.and_where(
								Expr::col("id").eq(Expr::value(row.try_get::<Uuid>("point_id")?)),
							)
							.and_where(Expr::col("retired").eq(false))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut **lease.tx())
					.await?
					.flatten();
					reusable = digest.as_deref()
						== Some(
							aidash_domain::semantic::indexing::content_digest(&unit.content.text)
								.as_str(),
						);
					if reusable {
						reusable = native::query(
							&Query::select()
								.column(Alias::new("id"))
								.from(Alias::new("semantic_vectors"))
								.and_where(
									Expr::col("id")
										.eq(Expr::value(row.try_get::<Uuid>("point_id")?)),
								)
								.and_where(Expr::col("collection").eq(index.collection.as_str()))
								.to_string(PostgresQueryBuilder),
						)
						.fetch_optional(&mut **lease.tx())
						.await?
						.is_some();
					}
				}
			}
		}
		if !reusable {
			repository::project(&mut lease, unit, "memory-restore", Some(provider)).await?;
		}
	}
	native::query(
		&Query::update()
			.table(Alias::new("memory_candidates"))
			.value(Alias::new("text"), "")
			.value(Alias::new("state"), "rejected")
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
			.value(Alias::new("entities"), serde_json::json!([]))
			.value(Alias::new("evidence"), serde_json::json!([]))
			.value(Alias::new("links"), serde_json::json!([]))
			.value(
				Alias::new("mental_model"),
				Option::<serde_json::Value>::None,
			)
			.value(Alias::new("occurred_start"), Option::<DateTime<Utc>>::None)
			.value(Alias::new("occurred_end"), Option::<DateTime<Utc>>::None)
			.value(Alias::new("updated_at"), Utc::now())
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	let operations = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("memory_model_operations"))
		.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
		.to_owned();
	native::query(
		&Query::update()
			.table(Alias::new("memory_model_attempts"))
			.value(Alias::new("output"), Option::<serde_json::Value>::None)
			.value(Alias::new("state"), "purged")
			.and_where(Expr::col("operation_id").in_subquery(operations))
			.and_where(Expr::col("output").is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	lease.finish(Ok(())).await?;
	// Rebuild against live control/budget pools. Interrupted/reserved attempts
	// remain conservatively charged; a failed rebuild keeps the gate closed.
	let indexing = crate::bootstrap::semantic_indexing_repository(store);
	let transport = crate::bootstrap::semantic_transport(store);
	for unit in &contents {
		aidash_application::semantic::process(&indexing, &transport, unit.id).await?;
	}
	let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	let contents = selected(&mut lease, bank, policy.retention.max_unit_records).await?;
	lease.finish(Ok(())).await?;
	report.withheld = contents.iter().filter(|unit| !unit.visible()).count();
	for unit in &contents {
		if !unit.visible() {
			continue;
		}
		validate_writer(store, unit, policy).await?;
		let row = native::query(
			&Query::select()
				.columns(["point_id", "state", "metadata", "index_revision"].map(Alias::new))
				.from(Alias::new("semantic_entries"))
				.and_where(Expr::col("id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(store.pool.in_scope(&RECOVERY))
		.await?;
		let metadata: serde_json::Value = row.try_get("metadata")?;
		if row.try_get::<String>("state")? != "READY"
			|| metadata["unit_revision"].as_i64() != Some(unit.revision)
		{
			return Err(Error::SemanticUnavailable);
		}
		let mut tx = store.pool.begin(&RECOVERY).await?;
		let index =
			crate::semantic::models::SemanticIndexe::locked(&mut *tx, bank.workspace, false)
				.await?
				.ok_or(Error::SemanticUnavailable)?;
		if index.revision != row.try_get::<i64>("index_revision")? {
			return Err(Error::SemanticUnavailable);
		}
		tx.commit().await?;
		let config = index.configuration()?;
		if !crate::bootstrap::semantic_transport(store)
			.present(
				&config.vector,
				&index.collection,
				&index.tenant,
				&[row.try_get("point_id")?],
			)
			.await?
		{
			return Err(Error::SemanticUnavailable);
		}
		report.restored += 1;
	}
	if !recovery.reconstructed()? {
		recovery.gate(false, epoch)?;
	}
	Ok(report)
}

pub async fn prune_expired(store: &Store, directory: &Path) -> Result<()> {
	let archives = directory.join("archives");
	if !archives.exists() {
		return Ok(());
	}
	// Worker sweeps reuse startup verification and its local publication gate.
	// Full history replay holds the writer lock across remote I/O and belongs
	// to startup and explicit recovery, never routine local archive cleanup.
	let active = store
		.pool
		.memory_recovery()
		.ok_or(Error::SemanticUnavailable)?;
	active.require_serving()?;
	let epoch = external(store, directory)?.epoch()?;
	if active.epoch()? != epoch {
		return Err(Error::Forbidden);
	}
	let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	let result = prune(
		&mut lease,
		&archives,
		store.deployment.as_str(),
		epoch,
		Utc::now(),
	)
	.await
	.map(|_| ());
	lease.finish(result).await
}

/// Persistently close memory serving before Deployment Administrator recovery work.
pub async fn prepare_restore(store: &Store, directory: &Path) -> Result<Uuid> {
	let recovery = active_external(store, directory).await?;
	let epoch = recovery.load()?.epoch;
	recovery.gate(true, epoch)?;
	Ok(epoch)
}

pub struct Command;
#[async_trait::async_trait]
impl reinhardt::commands::CapabilityCommand for Command {
	fn cli(&self) -> clap::Command {
		clap::Command::new("memory-recovery")
			.about("Manage new-format memory-only backups and the independent deletion ledger")
			.arg(
				clap::Arg::new("action")
					.value_parser([
						"init",
						"init-if-missing",
						"backup",
						"restore",
						"prepare-restore",
						"status",
						"prune",
						"protect",
						"backup-all",
						"prune-remote",
						"reconstruct",
						"reconcile-all",
						"resume",
					])
					.required(true),
			)
			.arg(
				clap::Arg::new("directory")
					.long("directory")
					.value_parser(clap::value_parser!(PathBuf))
					.required(true),
			)
			.arg(
				clap::Arg::new("workspace")
					.long("workspace")
					.value_parser(clap::value_parser!(Uuid)),
			)
			.arg(clap::Arg::new("tenant").long("tenant"))
			.arg(
				clap::Arg::new("participant")
					.long("participant")
					.value_parser(clap::value_parser!(Uuid)),
			)
			.arg(
				clap::Arg::new("archive")
					.long("archive")
					.value_parser(clap::value_parser!(PathBuf)),
			)
			.arg(
				clap::Arg::new("json")
					.long("json")
					.action(clap::ArgAction::SetTrue),
			)
			.arg(
				clap::Arg::new("writers-fenced")
					.long("writers-fenced")
					.action(clap::ArgAction::SetTrue)
					.help("Attest all server, worker and maintenance floor writers are stopped"),
			)
			.arg(
				clap::Arg::new("report")
					.long("report")
					.value_parser(clap::value_parser!(PathBuf)),
			)
	}
	fn requirements(
		&self,
		_: &clap::ArgMatches,
	) -> Vec<reinhardt::commands::CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &reinhardt::commands::CapabilityContext,
	) -> reinhardt::commands::CommandResult<()> {
		let result: Result<()> = async {
			let settings = crate::config::startup::load_settings()?;
			// Recovery never changes the schema; `migrate` runs first as its owner.
			crate::bootstrap::require_migrated(&settings).await?;
			let config = crate::config::Config::from_settings(&settings)?;
			// The migration Job runs `init-if-missing` as the owner that owns every
			// table, so this maintenance command accepts that login. The server role
			// passes the same check, as it does for a manually run action.
			let database =
				crate::database::runtime::connect_maintenance(&config.database_url, 8).await?;
			let directory = matches
				.get_one::<PathBuf>("directory")
				.ok_or(Error::Forbidden)?;
			let store = attach(
				Store::from_pool(
					database.into_postgres().ok_or(Error::Forbidden)?,
					config.deployment.clone(),
				)
				.await?
				.with_dashboard_policy(config.dashboard_policy())
				.with_keyless_origins(settings.keyless_endpoints.origins()?)
				.with_hosted_model_origins(settings.deployment.hosted_origins()?)
				.with_data_protection_floor(settings.deployment.data_protection_floor),
				directory.clone(),
			)
			.await?;
			let action = matches.get_one::<String>("action").map(String::as_str);
			if matches!(action, Some("protect" | "reconstruct" | "reconcile-all" | "resume")) && !matches.get_flag("writers-fenced") {
				return Err(Error::Invalid("supervised recovery requires --writers-fenced after stopping every floor writer".into()));
			}
			let body_objects = || -> Result<Arc<dyn aidash_application::ports::memory::objects::RecoveryObjects>> {
				let bucket = std::env::var("AIDASH_MEMORY_RECOVERY_ARCHIVE_BUCKET").map_err(|_| Error::Invalid("recovery archive bucket is required".into()))?;
				Ok(Arc::new(aidash_integrations::recovery::GcsObjects::new(bucket)?))
			};
			match action {
				Some("protect") => {
					protect(&store, directory).await?;
					println!("independent Home ledger protection verified");
				}
				Some("backup-all") => {
					let report = protection::backup_all(&store, directory, body_objects()?, protection::SpoolLimits::from_env()?).await?;
					println!("{}", serde_json::to_string(&report)?);
					if !report.complete { return Err(Error::Conflict("all-bank memory backup incomplete".into())); }
				}
				Some("prune-remote") => {
					let report = protection::prune_remote(&store, body_objects()?).await?;
					println!("{}", serde_json::to_string(&report)?);
					if !report.complete { return Err(Error::Conflict("remote memory archive pruning incomplete".into())); }
				}
				Some("reconstruct") => {
					println!("{}", serde_json::to_string(&reconstruct(&store, directory).await?)?);
				}
				Some("reconcile-all") => {
					let path = matches.get_one::<PathBuf>("report").ok_or_else(|| Error::Invalid("reconcile-all requires --report".into()))?;
					let report = protection::reconcile_all(&store, directory, body_objects()?).await?;
					recovery::atomic_write(path, &report)?;
					println!("{}", serde_json::to_string(&report)?);
				}
				Some("resume") => {
					let path = matches.get_one::<PathBuf>("report").ok_or_else(|| Error::Invalid("resume requires --report".into()))?;
					let report: protection::HomeReport = recovery::read(path)?;
					protection::resume(&store, directory, body_objects()?, &report).await?;
					println!("Home memory resumed with verified all-bank evidence");
				}
				Some("init") => {
					initialize(&store, directory.clone()).await?;
					println!("initialized new-format memory ledger");
				}
				Some("init-if-missing") => {
					initialize_if_missing(&store, directory.clone()).await?;
					println!("verified new-format memory ledger");
				}
				Some("backup") => {
					let workspace = *matches
						.get_one::<Uuid>("workspace")
						.ok_or_else(|| Error::Invalid("backup requires --workspace".into()))?;
					let bank = Bank {
						home: workspace_home(&store, workspace).await?,
						tenant: matches
							.get_one::<String>("tenant")
							.ok_or_else(|| Error::Invalid("backup requires --tenant".into()))?
							.clone(),
						workspace,
						participant: matches.get_one::<Uuid>("participant").copied(),
					};
					println!("{}", backup(&store, directory, bank).await?.display());
				}
				Some("restore") => {
					let archive = matches
						.get_one::<PathBuf>("archive")
						.ok_or_else(|| Error::Invalid("restore requires --archive".into()))?;
					println!(
						"{}",
						serde_json::to_string(&restore(&store, directory, archive).await?)?
					);
				}
				Some("prune") => {
					prune_expired(&store, directory).await?;
					println!("expired memory archives removed");
				}
				Some("prepare-restore") => {
					println!("{}", prepare_restore(&store, directory).await?);
				}
				Some("status") => {
					let recovery = external(&store, directory)?;
					let ledger = recovery.load()?;
					if matches.get_flag("json") {
						let _ = recovery.verify().await;
						println!("{}", serde_json::json!({
							"home": store.deployment.as_str(), "epoch": ledger.epoch, "restoring": ledger.restoring,
							"serving": recovery.require_serving().is_ok(), "fenced_units": ledger.units.len(),
							"protected": recovery.protection_requested()?, "head": recovery.publication_head().ok(),
						}));
					} else {
						println!("epoch={} restoring={} fenced_units={}", ledger.epoch, ledger.restoring, ledger.units.len());
					}
				}
				_ => return Err(Error::Forbidden),
			}
			Ok(())
		}
		.await;
		if result.is_err() {
			eprintln!(
				"{}",
				serde_json::json!({"event": "memory_recovery_command_failed", "action": matches.get_one::<String>("action")})
			);
		}
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}

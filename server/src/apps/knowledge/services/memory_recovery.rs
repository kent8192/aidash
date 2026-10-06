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
pub fn attach(store: Store, directory: PathBuf) -> Result<Store> {
	let ledger = Arc::new(FileRecovery::new(directory, store.node_id.clone())?);
	Ok(store.with_memory_recovery(Some(ledger)))
}
fn external(store: &Store, directory: &Path) -> Result<FileRecovery> {
	FileRecovery::new(directory.to_owned(), store.node_id.clone())
}
fn active_external(store: &Store, directory: &Path) -> Result<FileRecovery> {
	let recovery = external(store, directory)?;
	let active = store
		.pool
		.memory_recovery()
		.ok_or(Error::SemanticUnavailable)?;
	if active.epoch()? != recovery.load()?.epoch {
		return Err(Error::Forbidden);
	}
	Ok(recovery)
}
fn bypass(store: &Store) -> Store {
	let mut internal = store.clone();
	internal.pool = internal.pool.memory_recovery_internal();
	internal.control_pool = internal.control_pool.memory_recovery_internal();
	internal
}

/// Explicit first initialization of the new relational format, never a JSON import.
pub async fn initialize(store: &Store, directory: PathBuf) -> Result<Store> {
	let recovery = external(store, &directory)?;
	let internal = bypass(store);
	let mut lease =
		Lease::begin(&internal, &crate::authorization::identity::Actor::Operator).await?;
	let result = async {
		for table in ["memory_banks","memory_participants"] {
			if native::query(&Query::select().column(Alias::new("id")).from(Alias::new(table))
				.and_where(Expr::col("home").eq(store.node_id.as_str())).limit(1).to_string(PostgresQueryBuilder))
				.fetch_optional(&mut **lease.tx()).await?.is_some() {
				return Err(Error::Conflict("a new recovery epoch requires empty new-format memory; restore the existing external ledger".into()));
			}
		}
		recovery.initialize(&[])
	}
	.await;
	lease.finish(result).await?;
	attach(store.clone(), directory)
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
	let policy = native_memory::policy(lease, &settings.provider).await?;
	Ok((settings, policy))
}

/// Backups live in the external managed directory and expire under the pinned bank policy.
pub async fn backup(store: &Store, directory: &Path, bank: Bank) -> Result<PathBuf> {
	let recovery = active_external(store, directory)?;
	recovery.require_serving()?;
	let mut lease = Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
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
			home: store.node_id.clone(),
			epoch: ledger.epoch,
			bank,
			provider: settings.provider,
			settings_revision: settings.revision,
			created_at: now,
			expires_at: now + Duration::days(i64::from(policy.retention.backup_days)),
			units: contents,
		};
		let archives = directory.join("archives");
		std::fs::create_dir_all(&archives)?;
		if prune(&archives, now)? >= MAX_ARCHIVES {
			return Err(Error::Conflict(
				"managed memory backup capacity reached".into(),
			));
		}
		let path = archives.join(format!("{}.cbor", Uuid::new_v4()));
		recovery::atomic_write(&path, &archive)?;
		Ok(path)
	}
	.await;
	lease.finish(result).await
}

fn prune(directory: &Path, now: DateTime<Utc>) -> Result<usize> {
	let mut count = 0;
	for entry in std::fs::read_dir(directory)? {
		let entry = entry?;
		let path = entry.path();
		if path.extension().is_none_or(|extension| extension != "cbor") {
			continue;
		}
		let archive: Archive = recovery::read(&path)?;
		if archive.expires_at <= now {
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
	let recovery = active_external(store, directory)?;
	let archive: Archive = recovery::read(path)?;
	let ledger = recovery.load()?;
	if archive.format != 1
		|| archive.home != store.node_id
		|| archive.bank.home != store.node_id
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
	let mut lease =
		Lease::begin(&internal, &crate::authorization::identity::Actor::Operator).await?;
	let result = async {
		repository::lock_workspace(&mut lease, archive.bank.workspace, true).await?;
		let (settings, policy) = configuration(&mut lease, &archive.bank).await?;
		if settings.revision != archive.settings_revision
			|| settings.provider != archive.provider
			|| archive.units.len() > policy.retention.max_unit_records
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
				recovery.advance_restore(ledger.epoch, &unit)?;
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
	let mut lookup = Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
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
		.fetch_one(&store.pool)
		.await?;
		if record.home_node != unit.bank.home || record.workspace_id != unit.bank.workspace {
			return Err(Error::Forbidden);
		}
		if let Some(mut access) =
			crate::authorization::execution::access_for_run(store, &record.metadata(), true).await?
		{
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
				native_memory::definition(&mut Lease::Inherited(&mut access), reference, kind)
					.await?;
			}
			access.finish(Ok(())).await?;
		}
	}
	let mut writer = Lease::restore(store, serde_json::to_value(origin.authority)?).await?;
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
	for pass in 0..=policy.bounds.max_graph_hops {
		let mut lease =
			Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
		let contents = selected(&mut lease, bank, policy.retention.max_unit_records).await?;
		lease.finish(Ok(())).await?;
		let mut changed = false;
		for mut unit in contents.into_iter().filter(Unit::visible) {
			let ttl = policy.retention.unit_expired(unit.learned_at, Utc::now());
			let fenced = recovery.load()?.current(&unit)?;
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
			unit.revision = unit
				.revision
				.checked_add(1)
				.filter(|revision| *revision < i64::MAX)
				.ok_or(Error::Forbidden)?;
			unit.deleted = ttl;
			unit.stale = !ttl;
			unit.updated_at = Utc::now();
			clear(&mut unit);
			recovery.advance_restore(epoch, &unit)?;
			let mut lease =
				Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
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
		if pass == policy.bounds.max_graph_hops {
			return Err(Error::Conflict(
				"restore invalidation exceeds its declared traversal bound".into(),
			));
		}
	}
	let mut lease = Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
	repository::lock_workspace(&mut lease, bank.workspace, true).await?;
	let mut contents = selected(&mut lease, bank, policy.retention.max_unit_records).await?;
	let bank_id = repository::bank_id(&mut lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	for unit in &mut contents {
		if !unit.visible() {
			// A current stale unit may still carry a body pending ordinary
			// cleanup. The recovery gate opens only after removing that body.
			clear(unit);
			recovery.advance_restore(epoch, unit)?;
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
	let mut lease = Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
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
		.fetch_one(&store.pool)
		.await?;
		let metadata: serde_json::Value = row.try_get("metadata")?;
		if row.try_get::<String>("state")? != "READY"
			|| metadata["unit_revision"].as_i64() != Some(unit.revision)
		{
			return Err(Error::SemanticUnavailable);
		}
		let mut tx = store.pool.begin().await?;
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
				&[row.try_get("point_id")?],
			)
			.await?
		{
			return Err(Error::SemanticUnavailable);
		}
		report.restored += 1;
	}
	recovery.gate(false, epoch)?;
	Ok(report)
}

pub fn prune_expired(store: &Store, directory: &Path) -> Result<()> {
	let recovery = active_external(store, directory)?;
	recovery.load()?;
	let archives = directory.join("archives");
	if archives.exists() {
		prune(&archives, Utc::now())?;
	}
	Ok(())
}

/// Persistently close memory serving before operator recovery work.
pub fn prepare_restore(store: &Store, directory: &Path) -> Result<Uuid> {
	let recovery = active_external(store, directory)?;
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
						"backup",
						"restore",
						"prepare-restore",
						"status",
						"prune",
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
			crate::bootstrap::migrate(&settings).await?;
			let config = crate::config::Config::from_settings(&settings)?;
			let database =
				reinhardt::db::backends::DatabaseConnection::connect_postgres_with_pool_size(
					&config.database_url,
					Some(8),
				)
				.await?;
			let directory = matches
				.get_one::<PathBuf>("directory")
				.ok_or(Error::Forbidden)?;
			let store = attach(
				Store::from_pool(
					database.into_postgres().ok_or(Error::Forbidden)?,
					config.node_id,
				)
				.await?,
				directory.clone(),
			)?;
			match matches.get_one::<String>("action").map(String::as_str) {
				Some("init") => {
					initialize(&store, directory.clone()).await?;
					println!("initialized new-format memory ledger");
				}
				Some("backup") => {
					let bank = Bank {
						home: store.node_id.clone(),
						tenant: matches
							.get_one::<String>("tenant")
							.ok_or_else(|| Error::Invalid("backup requires --tenant".into()))?
							.clone(),
						workspace: *matches
							.get_one::<Uuid>("workspace")
							.ok_or_else(|| Error::Invalid("backup requires --workspace".into()))?,
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
					prune_expired(&store, directory)?;
					println!("expired memory archives removed");
				}
				Some("prepare-restore") => {
					println!("{}", prepare_restore(&store, directory)?);
				}
				Some("status") => {
					let ledger = external(&store, directory)?.load()?;
					println!(
						"epoch={} restoring={} fenced_units={}",
						ledger.epoch,
						ledger.restoring,
						ledger.units.len()
					);
				}
				_ => return Err(Error::Forbidden),
			}
			Ok(())
		}
		.await;
		result.map_err(|error| reinhardt::commands::CommandError::ExecutionError(error.to_string()))
	}
}

//! Scheduled body copies and supervised Home-wide recovery. Safety floors come
//! exclusively from the independent history, never from these body archives.
use super::*;
use aidash_application::ports::memory::objects::RecoveryObjects;
use aidash_domain::memory::recovery::history::Head;
use std::{
	collections::{BTreeMap, BTreeSet},
	fs::File,
};

#[derive(Debug, Clone, Copy)]
pub struct SpoolLimits {
	pub files: usize,
	pub bytes: u64,
}
impl Default for SpoolLimits {
	fn default() -> Self {
		Self {
			files: 128,
			bytes: 512 * 1024 * 1024,
		}
	}
}
impl SpoolLimits {
	pub fn from_env() -> Result<Self> {
		let defaults = Self::default();
		let limits = Self {
			files: std::env::var("AIDASH_MEMORY_RECOVERY_SPOOL_MAX_FILES")
				.ok()
				.map(|value| value.parse())
				.transpose()
				.map_err(|_| Error::Invalid("invalid recovery spool file limit".into()))?
				.unwrap_or(defaults.files),
			bytes: std::env::var("AIDASH_MEMORY_RECOVERY_SPOOL_MAX_BYTES")
				.ok()
				.map(|value| value.parse())
				.transpose()
				.map_err(|_| Error::Invalid("invalid recovery spool byte limit".into()))?
				.unwrap_or(defaults.bytes),
		};
		if limits.files == 0 || limits.bytes < recovery::MAX_FILE_BYTES {
			return Err(Error::Invalid(
				"recovery spool must fit at least one maximum-size archive".into(),
			));
		}
		Ok(limits)
	}
	pub(super) fn require_capacity(&self, directory: &Path, archive: &Archive) -> Result<()> {
		let mut bytes = Vec::new();
		ciborium::ser::into_writer(archive, &mut bytes).map_err(|_| Error::SemanticUnavailable)?;
		let size = bytes.len() as u64 + 40;
		if size > recovery::MAX_FILE_BYTES {
			return Err(Error::Invalid(
				"memory recovery archive exceeds its file cap".into(),
			));
		}
		let mut total = 0u64;
		let mut count = 0usize;
		for item in std::fs::read_dir(directory)? {
			let item = item?;
			if !item.file_type()?.is_file() {
				return Err(Error::Invalid("unexpected recovery spool entry".into()));
			}
			count += 1;
			total = total
				.checked_add(item.metadata()?.len())
				.ok_or(Error::SemanticUnavailable)?;
		}
		if count >= self.files
			|| total
				.checked_add(size)
				.is_none_or(|value| value > self.bytes)
		{
			tracing::error!(
				event = "memory_recovery_spool_exhausted",
				"memory recovery spool exhausted"
			);
			return Err(Error::Conflict("managed recovery spool exhausted".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BankState {
	pub id: Uuid,
	pub bank: Bank,
	pub provider: EntityRef,
	pub revision: i64,
	pub eligible_at: DateTime<Utc>,
}
/// Stable UUID keyset pages bounded by both the observed ID and eligibility cut.
async fn inventory(store: &Store, cut: DateTime<Utc>) -> Result<Vec<BankState>> {
	let mut lease = Lease::deployment(store, DeploymentScopeReason::MemoryRecovery).await?;
	let mut after = Uuid::nil();
	let mut result = Vec::new();
	loop {
		let rows = native::query(
			&Query::select()
				.columns(
					["id", "home", "tenant", "workspace_id", "participant_id"]
						.map(|column| (Alias::new("b"), Alias::new(column))),
				)
				.columns(
					[
						"provider_id",
						"provider_version",
						"revision",
						"recovery_eligible_at",
					]
					.map(|column| (Alias::new("memory_bank_settings"), Alias::new(column))),
				)
				.from_as(Alias::new("memory_banks"), Alias::new("b"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					Alias::new("memory_bank_settings"),
					Expr::col((Alias::new("memory_bank_settings"), Alias::new("bank_id")))
						.equals((Alias::new("b"), Alias::new("id"))),
				)
				.and_where(crate::database::scope::in_deployment(
					"b.home",
					store.deployment.as_str(),
				))
				.and_where(Expr::col((Alias::new("b"), Alias::new("id"))).gt(Expr::value(after)))
				.and_where(
					Expr::col((
						Alias::new("memory_bank_settings"),
						Alias::new("recovery_eligible_at"),
					))
					.lte(Expr::value(cut)),
				)
				.order_by((Alias::new("b"), Alias::new("id")), Order::Asc)
				.limit(128)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut **lease.tx())
		.await?;
		let length = rows.len();
		for row in rows {
			after = row.try_get("id")?;
			result.push(BankState {
				id: after,
				bank: Bank {
					home: row.try_get("home")?,
					tenant: row.try_get("tenant")?,
					workspace: row.try_get("workspace_id")?,
					participant: row.try_get("participant_id")?,
				},
				provider: EntityRef {
					id: row.try_get("provider_id")?,
					version: row.try_get("provider_version")?,
				},
				revision: row.try_get("revision")?,
				eligible_at: row.try_get("recovery_eligible_at")?,
			});
		}
		if length < 128 {
			break;
		}
	}
	lease.finish(Ok(result)).await
}
fn retired(error: &Error) -> bool {
	matches!(
		error,
		Error::Forbidden
			| Error::Unauthorized
			| Error::NotFound(_)
			| Error::TenantDeleted
			| Error::Conflict(_)
			| Error::Invalid(_)
	)
}
async fn live_policy(
	store: &Store,
	bank: &Bank,
) -> Result<Option<(bank_settings::Settings, Policy)>> {
	let internal = bypass(store);
	let mut lease = Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
	let result = configuration(&mut lease, bank).await;
	match lease.finish(result).await {
		Ok(value) => Ok(Some(value)),
		Err(error) if retired(&error) => Ok(None),
		Err(error) => Err(error),
	}
}
async fn eligible(
	store: &Store,
	archive: &Archive,
	epoch: Option<Uuid>,
	now: DateTime<Utc>,
) -> Result<bool> {
	if archive.format != 1
		|| archive.home != store.deployment
		|| aidash_domain::nodes::deployment_of(&archive.bank.home) != store.deployment
		|| epoch.is_some_and(|value| archive.epoch != value)
		|| archive.epoch.is_nil()
		|| archive.created_at > now
		|| archive.created_at >= archive.expires_at
		|| archive.expires_at <= now
	{
		return Ok(false);
	}
	let Some((settings, policy)) = live_policy(store, &archive.bank).await? else {
		return Ok(false);
	};
	Ok(settings.revision == archive.settings_revision
		&& settings.provider == archive.provider
		&& archive.units.len() <= policy.retention.max_unit_records
		&& archive.created_at + Duration::days(i64::from(policy.retention.backup_days)) > now)
}
fn prefix(home: &str) -> String {
	format!(
		"homes/{}/archives/",
		recovery::history::digest(home.as_bytes())
	)
}
async fn objects(
	store: &dyn RecoveryObjects,
	prefix: &str,
) -> Result<Vec<aidash_application::ports::memory::objects::ObjectName>> {
	let mut seen = BTreeSet::new();
	let mut next = None;
	let mut names = Vec::new();
	loop {
		let page = store.list(prefix, next.as_deref()).await?;
		for object in page.objects {
			if !object.name.starts_with(prefix) {
				return Err(Error::Forbidden);
			}
			names.push(object);
		}
		next = page.next;
		match &next {
			None => break,
			Some(token) if seen.insert(token.clone()) => {}
			Some(_) => return Err(Error::SemanticUnavailable),
		}
	}
	Ok(names)
}
#[derive(Debug, Serialize)]
pub struct BankOutcome {
	pub bank: Uuid,
	pub state: &'static str,
}
#[derive(Debug, Serialize)]
pub struct BackupReport {
	pub event: &'static str,
	pub home: String,
	pub cut: DateTime<Utc>,
	pub complete: bool,
	pub banks: Vec<BankOutcome>,
}

async fn transfer(
	store: &Store,
	remote: &dyn RecoveryObjects,
	directory: &Path,
	path: &Path,
) -> Result<()> {
	let bytes = recovery::bytes(path)?;
	let archive: Archive = recovery::decode(&bytes)?;
	if !eligible(
		store,
		&archive,
		Some(external(store, directory)?.load()?.epoch),
		Utc::now(),
	)
	.await?
	{
		std::fs::remove_file(path)?;
		return Err(Error::Conflict(
			"scheduled recovery archive is no longer eligible".into(),
		));
	}
	let name = format!(
		"{}{}",
		prefix(store.deployment.as_str()),
		path.file_name()
			.and_then(|name| name.to_str())
			.ok_or(Error::Forbidden)?
	);
	let generation = remote.create(&name, &bytes).await?;
	let saved = remote
		.get(&name, Some(&generation))
		.await?
		.ok_or(Error::SemanticUnavailable)?;
	if saved.bytes != bytes || !eligible(store, &archive, None, Utc::now()).await? {
		return Err(Error::SemanticUnavailable);
	}
	std::fs::remove_file(path)?;
	File::open(path.parent().ok_or(Error::Forbidden)?)?.sync_all()?;
	Ok(())
}
/// Continue after a failed bank; a successful receipt accounts for the entire cut.
pub async fn backup_all(
	store: &Store,
	directory: &Path,
	remote: Arc<dyn RecoveryObjects>,
	limits: SpoolLimits,
) -> Result<BackupReport> {
	let cut = Utc::now();
	let banks = inventory(store, cut).await?;
	let spool = directory.join("spool");
	std::fs::create_dir_all(&spool)?;
	let lock = std::fs::OpenOptions::new()
		.create(true)
		.truncate(false)
		.write(true)
		.open(directory.join("spool.lock"))?;
	lock.try_lock()
		.map_err(|_| Error::Conflict("scheduled memory backup is already running".into()))?;
	let mut pending = Vec::new();
	// Invalid unsent files expire independently of transfer success.
	for entry in std::fs::read_dir(&spool)? {
		let entry = entry?;
		let path = entry.path();
		// Only this locked scheduler writes the spool. A killed atomic write
		// has no committed archive; reclaim its recognized regular temporary.
		let abandoned = path
			.file_name()
			.and_then(|name| name.to_str())
			.and_then(|name| name.strip_prefix(".memory-"))
			.and_then(|name| name.strip_suffix(".pending"))
			.is_some_and(|id| Uuid::parse_str(id).is_ok());
		if abandoned && entry.file_type()?.is_file() {
			std::fs::remove_file(path)?;
			continue;
		}
		if path.extension().is_none_or(|ext| ext != "cbor") {
			continue;
		}
		let valid = match recovery::read::<Archive>(&path) {
			Ok(archive) => {
				if eligible(store, &archive, None, cut).await? {
					pending.push((path.clone(), archive.bank));
					true
				} else {
					false
				}
			}
			Err(_) => false,
		};
		if !valid {
			std::fs::remove_file(path)?;
		}
	}
	File::open(&spool)?.sync_all()?;
	let mut report = BackupReport {
		event: "memory_recovery_archive_succeeded",
		home: store.deployment.to_string(),
		cut,
		complete: true,
		banks: Vec::new(),
	};
	for bank in banks {
		let result: Result<&'static str> = async {
			if live_policy(store, &bank.bank).await?.is_none() {
				return Ok("retired");
			}
			// One retry per bank per run leaves time for later banks. A valid unsent
			// file is retained on error, never evicted to fit a replacement.
			if let Some((path, _)) = pending.iter().find(|(_, selected)| selected == &bank.bank) {
				transfer(store, remote.as_ref(), directory, path).await?;
			}
			let path = backup_to(store, directory, bank.bank.clone(), Some(limits)).await?;
			transfer(store, remote.as_ref(), directory, &path).await?;
			Ok("verified")
		}
		.await;
		let state = match result {
			Ok(state) => state,
			Err(_) => {
				report.complete = false;
				report.event = "memory_recovery_archive_failed";
				tracing::error!(event = "memory_recovery_archive_failed", bank_id = %bank.id, "memory recovery archive failed");
				"failed"
			}
		};
		report.banks.push(BankOutcome {
			bank: bank.id,
			state,
		});
	}
	Ok(report)
}

#[derive(Debug, Serialize)]
pub struct PruneReport {
	pub event: &'static str,
	pub stale_count: usize,
	pub complete: bool,
	pub deleted: usize,
	pub failed: usize,
	pub stale: Vec<Uuid>,
}
/// No local filesystem or native-memory gate is consulted here.
pub async fn prune_remote(store: &Store, remote: Arc<dyn RecoveryObjects>) -> Result<PruneReport> {
	let now = Utc::now();
	let banks = inventory(store, now).await?;
	let mut report = PruneReport {
		event: "memory_recovery_prune_succeeded",
		stale_count: 0,
		complete: true,
		deleted: 0,
		failed: 0,
		stale: Vec::new(),
	};
	let mut latest = BTreeMap::<Uuid, DateTime<Utc>>::new();
	for item in objects(remote.as_ref(), &prefix(store.deployment.as_str())).await? {
		let result: Result<()> = async {
			let Some(object) = remote.get(&item.name, Some(&item.generation)).await? else {
				return Ok(());
			};
			let archive = recovery::decode::<Archive>(&object.bytes);
			let valid = match &archive {
				// A concurrent backup can upload after this pass captured its cut.
				// Defer that copy; temporal ineligibility at an older cut is not expiry.
				Ok(archive) if archive.created_at > now => return Ok(()),
				Ok(archive) => eligible(store, archive, None, now).await?,
				Err(_) => false,
			};
			if !valid {
				remote.delete(&item.name, &item.generation).await?;
				report.deleted += 1;
			} else if let Ok(archive) = archive
				&& let Some(bank) = banks.iter().find(|bank| bank.bank == archive.bank)
			{
				latest
					.entry(bank.id)
					.and_modify(|value| *value = (*value).max(archive.created_at))
					.or_insert(archive.created_at);
			}
			Ok(())
		}
		.await;
		if result.is_err() {
			report.failed += 1;
			report.complete = false;
		}
	}
	for bank in banks {
		if live_policy(store, &bank.bank).await?.is_none() {
			continue;
		}
		let since = latest.get(&bank.id).copied().unwrap_or(bank.eligible_at);
		if now - since > Duration::hours(8) {
			report.stale.push(bank.id);
			tracing::error!(event = "memory_recovery_archive_stale", bank_id = %bank.id, age_seconds = (now-since).num_seconds(), "memory recovery verified archive exceeds eight hours");
		}
	}
	report.stale_count = report.stale.len();
	if report.complete {
		tracing::info!(
			event = "memory_recovery_prune_succeeded",
			"memory recovery remote pruning succeeded"
		);
	} else {
		report.event = "memory_recovery_prune_failed";
		tracing::error!(
			event = "memory_recovery_prune_failed",
			"memory recovery remote pruning failed"
		);
	}
	Ok(report)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeReport {
	pub archive_unavailable: bool,
	pub format: u32,
	pub home: String,
	pub epoch: Uuid,
	pub head: Head,
	pub banks: Vec<BankState>,
	pub restored: usize,
	pub withheld: usize,
}
/// Reconcile current bodies first. Archive bodies are admitted only when their
/// exact identities and digests match the independently reconstructed floors.
pub async fn reconcile_all(
	store: &Store,
	directory: &Path,
	remote: Arc<dyn RecoveryObjects>,
) -> Result<HomeReport> {
	let recovery = active_external(store, directory).await?;
	if !recovery.reconstructed()? {
		return Err(Error::Conflict(
			"Home reconciliation requires a reconstructed closed ledger".into(),
		));
	}
	let internal = bypass(store);
	let initial = recovery.load()?;
	let banks = inventory(&internal, Utc::now()).await?;
	let mut archives = Vec::new();
	let mut archive_attempted = false;
	let mut report = HomeReport {
		archive_unavailable: false,
		format: 1,
		home: store.deployment.to_string(),
		epoch: initial.epoch,
		head: recovery.publication_head()?,
		banks: banks.clone(),
		restored: 0,
		withheld: 0,
	};
	for bank in &banks {
		let Some((settings, policy)) = live_policy(&internal, &bank.bank).await? else {
			// Retired banks remain inaccessible under live authority. Their floors
			// stay in the history even when no current bank may serve their bodies.
			report.withheld += initial
				.units
				.values()
				.filter(|floor| floor.bank == bank.bank)
				.count();
			continue;
		};
		if settings.revision != bank.revision || settings.provider != bank.provider {
			return Err(Error::Conflict(
				"bank inventory changed during Home reconciliation".into(),
			));
		}
		if !archive_attempted {
			let mut snapshot =
				Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
			let current =
				selected(&mut snapshot, &bank.bank, policy.retention.max_unit_records).await?;
			snapshot.finish(Ok(())).await?;
			let needed = initial.units.iter().any(|(id, floor)| {
				floor.bank == bank.bank
					&& !floor.deleted
					&& !current
						.iter()
						.any(|unit| unit.id == *id && initial.matches(unit).unwrap_or(false))
			});
			if needed {
				archive_attempted = true;
				let copied: Result<Vec<Archive>> = async {
					let mut result = Vec::new();
					for item in objects(remote.as_ref(), &prefix(store.deployment.as_str())).await?
					{
						let Some(object) = remote.get(&item.name, Some(&item.generation)).await?
						else {
							continue;
						};
						let Ok(archive) = recovery::decode::<Archive>(&object.bytes) else {
							continue;
						};
						if eligible(&internal, &archive, Some(initial.epoch), Utc::now()).await? {
							result.push(archive);
						}
					}
					Ok(result)
				}
				.await;
				match copied {
					Ok(copied) => archives = copied,
					Err(_) => report.archive_unavailable = true,
				}
			}
		}
		let mut lease = Lease::deployment(&internal, DeploymentScopeReason::MemoryRecovery).await?;
		let result: Result<()> = async {
			repository::lock_workspace(&mut lease, bank.bank.workspace, true).await?;
			let id = repository::bank_id(&mut lease, &bank.bank, false)
				.await?
				.ok_or(Error::Forbidden)?;
			let current =
				selected(&mut lease, &bank.bank, policy.retention.max_unit_records).await?;
			for unit in &current {
				if !initial.units.contains_key(&unit.id) {
					return Err(Error::SemanticUnavailable);
				}
			}
			for (unit_id, floor) in initial
				.units
				.iter()
				.filter(|(_, floor)| floor.bank == bank.bank)
			{
				let current = units::load(&mut lease, *unit_id, true).await?;
				if current
					.as_ref()
					.is_some_and(|unit| initial.matches(unit).unwrap_or(false))
				{
					continue;
				}
				let copied = archives
					.iter()
					.filter(|archive| archive.bank == bank.bank)
					.flat_map(|archive| &archive.units)
					.find(|unit| unit.id == *unit_id && initial.current(unit).unwrap_or(false));
				if let Some(unit) = copied {
					let origin = unit_origins::load(&mut lease, unit.id).await?;
					if origin.is_some_and(|origin| origin.revision == floor.revision) {
						repository::restore_body(&mut lease, id, unit).await?;
					}
				}
				// Absent SQL identities with no matching body remain absent. The
				// conservative floor is retained and is never fabricated from SQL.
			}
			Ok(())
		}
		.await;
		lease.finish(result).await?;
		let result = finish(
			&internal,
			&recovery,
			&bank.bank,
			&bank.provider,
			initial.epoch,
			&policy,
		)
		.await?;
		report.restored += result.restored;
		report.withheld += result.withheld;
	}
	if inventory(&internal, Utc::now()).await? != banks {
		return Err(Error::Conflict(
			"bank inventory changed during Home reconciliation".into(),
		));
	}
	report.head = recovery.publication_head()?;
	// Floors for absent/retired/unconfigured SQL identities are still accounted
	// as withheld. Their negative proof is retained without manufacturing bodies.
	report.withheld = initial
		.units
		.len()
		.checked_sub(report.restored)
		.ok_or(Error::SemanticUnavailable)?;
	Ok(report)
}
/// Recheck current policy/source/authority and complete inventory immediately
/// before claiming Resume; any new withholding changes the evidence head.
pub async fn resume(
	store: &Store,
	directory: &Path,
	remote: Arc<dyn RecoveryObjects>,
	report: &HomeReport,
) -> Result<()> {
	if report.format != 1 || report.home != store.deployment {
		return Err(Error::Forbidden);
	}
	let recovery = active_external(store, directory).await?;
	let ledger = recovery.load()?;
	if ledger.epoch != report.epoch {
		return Err(Error::Forbidden);
	}
	if recovery.reconstructed()? && recovery.publication_head()?.recovering {
		let current = reconcile_all(store, directory, remote).await?;
		if current.head != report.head
			|| current.banks != report.banks
			|| current.restored != report.restored
			|| current.withheld != report.withheld
		{
			return Err(Error::Conflict(
				"Home reconciliation report is no longer current; save a fresh report".into(),
			));
		}
	}
	let evidence = recovery::history::digest(&serde_json::to_vec(report)?);
	recovery.resume_reconstructed(&report.head, evidence).await
}

//! Independent immutable history and durable local publication state.
use super::*;
use aidash_application::ports::memory::objects::RecoveryObjects;
use aidash_domain::memory::recovery::history::{Change, Floor, Head, History, Record};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
};
use uuid::Uuid;

pub(super) const REQUIRED: &str = "protection-required";
const STATE: &str = "publication.cbor";
const PENDING: &str = "publication-pending.cbor";

fn notify_closed(reason: &'static str) {
	tracing::error!(
		event = "memory_recovery_closed",
		reason,
		"Home memory protection requires reconciliation"
	);
	// Native commands and embedding hosts may own a plain-text subscriber. Keep
	// the production log alert independent of its format, without serializing errors.
	eprintln!(
		"{}",
		serde_json::json!({"event": "memory_recovery_closed", "reason": reason})
	);
}
const ACTIVATION: &str = "protection-activation.cbor";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
	epoch: Uuid,
	was_closed: bool,
}

#[derive(Clone)]
pub(crate) struct HistoryStore {
	pub objects: Arc<dyn RecoveryObjects>,
	home: String,
	prefix: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
	name: String,
	generation: String,
	digest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
	format: u32,
	home: String,
	epoch: Uuid,
	chunks: Vec<Chunk>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publication {
	pub epoch: Uuid,
	pub head: Head,
	pub generation: String,
	pub reconstructed: bool,
}
pub(crate) struct Replay {
	pub history: History,
	pub generation: String,
}

pub(crate) fn digest(bytes: &[u8]) -> String {
	format!("{:x}", Sha256::digest(bytes))
}

impl HistoryStore {
	pub fn new(home: String, objects: Arc<dyn RecoveryObjects>) -> Self {
		Self {
			prefix: format!("homes/{}", digest(home.as_bytes())),
			home,
			objects,
		}
	}
	pub fn key(&self, suffix: &str) -> String {
		format!("{}/{suffix}", self.prefix)
	}
	pub fn record_key(&self, sequence: u64) -> String {
		self.key(&format!("records/{sequence:020}.json"))
	}
	pub async fn seed(&self, ledger: &Ledger) -> Result<Replay> {
		ledger.validate(&self.home)?;
		let entries = ledger.units.iter().collect::<Vec<_>>();
		let mut chunks = Vec::new();
		for (index, part) in entries.chunks(1024).enumerate() {
			let bytes = serde_json::to_vec(part)?;
			let name = self.key(&format!("seed/{}/{index:020}.json", ledger.epoch));
			let generation = self.objects.create(&name, &bytes).await?;
			let saved = self
				.objects
				.get(&name, Some(&generation))
				.await?
				.ok_or(Error::SemanticUnavailable)?;
			if saved.bytes != bytes {
				return Err(Error::SemanticUnavailable);
			}
			chunks.push(Chunk {
				name,
				generation,
				digest: digest(&bytes),
			});
		}
		let seed = Seed {
			format: 1,
			home: self.home.clone(),
			epoch: ledger.epoch,
			chunks,
		};
		let bytes = serde_json::to_vec(&seed)?;
		let generation = self
			.objects
			.create(&self.key("anchor.json"), &bytes)
			.await?;
		let saved = self
			.objects
			.get(&self.key("anchor.json"), Some(&generation))
			.await?
			.ok_or(Error::SemanticUnavailable)?;
		if saved.bytes != bytes {
			return Err(Error::SemanticUnavailable);
		}
		Ok(Replay {
			history: History::new(ledger.clone(), digest(&bytes))?,
			generation,
		})
	}
	/// Listing establishes the complete committed inventory; an empty/missing slot
	/// in the middle is an error rather than a reason to accept a shorter tail.
	pub async fn replay(&self) -> Result<Replay> {
		let anchor = self
			.objects
			.get(&self.key("anchor.json"), None)
			.await?
			.ok_or(Error::SemanticUnavailable)?;
		let seed: Seed = serde_json::from_slice(&anchor.bytes)
			.map_err(|_| Error::Invalid("invalid memory recovery history object".into()))?;
		if seed.format != 1 || seed.home != self.home || seed.epoch.is_nil() {
			return Err(Error::Forbidden);
		}
		let mut ledger = Ledger::new(self.home.clone());
		ledger.epoch = seed.epoch;
		ledger.restoring = true;
		for (index, chunk) in seed.chunks.into_iter().enumerate() {
			if chunk.name != self.key(&format!("seed/{}/{index:020}.json", seed.epoch)) {
				return Err(Error::Forbidden);
			}
			let saved = self
				.objects
				.get(&chunk.name, Some(&chunk.generation))
				.await?
				.ok_or(Error::SemanticUnavailable)?;
			if digest(&saved.bytes) != chunk.digest {
				return Err(Error::SemanticUnavailable);
			}
			let entries: Vec<(Uuid, Fence)> = serde_json::from_slice(&saved.bytes)
				.map_err(|_| Error::Invalid("invalid memory recovery history object".into()))?;
			for (id, fence) in entries {
				if ledger.units.insert(id, fence).is_some() {
					return Err(Error::SemanticUnavailable);
				}
			}
		}
		let mut history = History::new(ledger, digest(&anchor.bytes))?;
		let prefix = self.key("records/");
		let mut pages = BTreeSet::new();
		let mut next = None;
		let mut inventory = BTreeMap::new();
		loop {
			let page = self.objects.list(&prefix, next.as_deref()).await?;
			for item in page.objects {
				let suffix = item
					.name
					.strip_prefix(&prefix)
					.and_then(|name| name.strip_suffix(".json"))
					.ok_or(Error::Forbidden)?;
				if suffix.len() != 20 || !suffix.bytes().all(|value| value.is_ascii_digit()) {
					return Err(Error::Forbidden);
				}
				let sequence: u64 = suffix.parse().map_err(|_| Error::Forbidden)?;
				if sequence == 0 || inventory.insert(sequence, item.generation).is_some() {
					return Err(Error::SemanticUnavailable);
				}
			}
			next = page.next;
			match &next {
				None => break,
				Some(token) if pages.insert(token.clone()) => {}
				Some(_) => return Err(Error::SemanticUnavailable),
			}
		}
		let mut generation = anchor.generation;
		for (sequence, saved_generation) in inventory {
			if sequence != history.head.sequence + 1 {
				return Err(Error::SemanticUnavailable);
			}
			let object = self
				.objects
				.get(&self.record_key(sequence), Some(&saved_generation))
				.await?
				.ok_or(Error::SemanticUnavailable)?;
			let record: Record = serde_json::from_slice(&object.bytes)
				.map_err(|_| Error::Invalid("invalid memory recovery history object".into()))?;
			if record.sequence != sequence {
				return Err(Error::Forbidden);
			}
			history.apply(&record)?;
			generation = saved_generation;
		}
		Ok(Replay {
			history,
			generation,
		})
	}
	pub async fn append(&self, record: &Record) -> Result<String> {
		Ok(self
			.objects
			.create(
				&self.record_key(record.sequence),
				&serde_json::to_vec(record)?,
			)
			.await?)
	}
	pub async fn confirm(&self, publication: &Publication) -> Result<()> {
		let name = if publication.head.sequence == 0 {
			self.key("anchor.json")
		} else {
			self.record_key(publication.head.sequence)
		};
		let object = self
			.objects
			.get(&name, Some(&publication.generation))
			.await?
			.ok_or(Error::SemanticUnavailable)?;
		let actual = if publication.head.sequence == 0 {
			digest(&object.bytes)
		} else {
			let record: Record = serde_json::from_slice(&object.bytes)
				.map_err(|_| Error::Invalid("invalid memory recovery history object".into()))?;
			if record.home != self.home || record.epoch != publication.epoch {
				return Err(Error::Forbidden);
			}
			record.digest()?
		};
		if actual != publication.head.digest {
			return Err(Error::SemanticUnavailable);
		}
		Ok(())
	}
	pub async fn barrier(&self) -> Result<Replay> {
		for _ in 0..32 {
			let mut replay = self.replay().await?;
			let record = replay.history.next(Uuid::new_v4(), Change::Barrier)?;
			match self.append(&record).await {
				Ok(generation) => {
					replay.history.apply(&record)?;
					replay.generation = generation;
					return Ok(replay);
				}
				Err(Error::Conflict(_)) => continue,
				Err(error) => return Err(error),
			}
		}
		Err(Error::Conflict(
			"memory writers have not been fenced".into(),
		))
	}
}

impl FileRecovery {
	pub fn with_objects(mut self, objects: Arc<dyn RecoveryObjects>) -> Self {
		self.independent = Some(Arc::new(HistoryStore::new(self.home.clone(), objects)));
		self.verified = Arc::new(AtomicBool::new(false));
		self
	}
	pub fn from_env(directory: PathBuf, home: String) -> Result<Self> {
		let mut recovery = Self::new(directory, home)?;
		recovery.required =
			std::env::var("AIDASH_MEMORY_RECOVERY_REQUIRED").is_ok_and(|value| value == "true");
		if let Some(bucket) = std::env::var("AIDASH_MEMORY_RECOVERY_HISTORY_BUCKET")
			.ok()
			.filter(|value| !value.is_empty())
		{
			recovery = recovery.with_objects(Arc::new(
				aidash_integrations::recovery::GcsObjects::new(bucket)?,
			));
		}
		Ok(recovery)
	}
	pub fn protection_requested(&self) -> Result<bool> {
		Ok(self.required
			|| self.independent.is_some()
			|| self
				.directory
				.join(REQUIRED)
				.try_exists()
				.map_err(storage_error)?)
	}
	fn publication(&self, epoch: Uuid) -> Result<Publication> {
		let state: Publication = read(&self.directory.join(STATE))?;
		if state.epoch != epoch || state.head.digest.is_empty() {
			return Err(Error::SemanticUnavailable);
		}
		Ok(state)
	}
	pub(super) fn require_publication(&self, epoch: Uuid) -> Result<()> {
		if self.protection_requested()? {
			if self.independent.is_none()
				|| !self.verified.load(Ordering::Acquire)
				|| self
					.directory
					.join(PENDING)
					.try_exists()
					.map_err(storage_error)?
			{
				return Err(Error::SemanticUnavailable);
			}
			let state = self.publication(epoch)?;
			if state.reconstructed || state.head.recovering {
				return Err(Error::SemanticUnavailable);
			}
		}
		Ok(())
	}
	pub(super) async fn waiting_lock(&self) -> Result<File> {
		let recovery = self.clone();
		tokio::task::spawn_blocking(move || recovery.lock_wait())
			.await
			.map_err(|_| Error::SemanticUnavailable)?
	}
	fn write_fences(&self, ledger: &Ledger) -> Result<()> {
		for (id, fence) in &ledger.units {
			atomic_write(
				&self.unit_path(*id),
				&UnitFence {
					epoch: ledger.epoch,
					id: *id,
					fence: fence.clone(),
				},
			)?;
		}
		Ok(())
	}
	fn clear_pending(&self) -> Result<()> {
		self.clear_named(PENDING)
	}
	fn clear_named(&self, name: &str) -> Result<()> {
		match std::fs::remove_file(self.directory.join(name)) {
			Ok(()) => File::open(&self.directory)
				.and_then(|file| file.sync_all())
				.map_err(storage_error),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
			Err(error) => Err(storage_error(error)),
		}
	}
	/// Startup and explicit repair may complete a positively confirmed pending
	/// publication, but never infer cancellation from a missing response or 404.
	pub async fn verify(&self) -> Result<()> {
		let result = self.verify_inner().await;
		if result.is_err() && self.protection_requested().unwrap_or(true) {
			notify_closed("verification_failed");
		}
		result
	}
	async fn verify_inner(&self) -> Result<()> {
		if !self.protection_requested()? {
			self.load()?;
			self.verified.store(true, Ordering::Release);
			return Ok(());
		}
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		let _guard = self.waiting_lock().await?;
		let local = self.load_units_locked(self.load_locked()?)?;
		let state = self.publication(local.epoch)?;
		let pending = self
			.directory
			.join(PENDING)
			.try_exists()
			.map_err(storage_error)?;
		if pending {
			self.verified.store(false, Ordering::Release);
			let record: Record = read(&self.directory.join(PENDING))?;
			if record.epoch != local.epoch || record.home != self.home {
				return Err(Error::Forbidden);
			}
			independent.append(&record).await?;
		}
		let replay = independent.replay().await?;
		if replay.history.ledger.epoch != local.epoch {
			return Err(Error::Forbidden);
		}
		if !pending
			&& (replay.history.head != state.head || replay.history.ledger.units != local.units)
		{
			self.verified.store(false, Ordering::Release);
			return Err(Error::SemanticUnavailable);
		}
		if pending {
			for (id, fence) in &local.units {
				let protected = replay
					.history
					.ledger
					.units
					.get(id)
					.ok_or(Error::SemanticUnavailable)?;
				if protected.bank != fence.bank
					|| protected.revision < fence.revision
					|| (fence.deleted && !protected.deleted)
				{
					return Err(Error::SemanticUnavailable);
				}
			}
			self.write_fences(&replay.history.ledger)?;
			atomic_write(
				&self.directory.join(STATE),
				&Publication {
					epoch: local.epoch,
					head: replay.history.head.clone(),
					generation: replay.generation,
					reconstructed: state.reconstructed || replay.history.head.recovering,
				},
			)?;
			self.clear_pending()?;
		}
		self.verified.store(true, Ordering::Release);
		Ok(())
	}
	pub(super) async fn initialize_publication(&self, ledger: &Ledger) -> Result<()> {
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		atomic_write(&self.directory.join(REQUIRED), &true)?;
		let replay = independent.seed(ledger).await?;
		atomic_write(
			&self.directory.join(STATE),
			&Publication {
				epoch: ledger.epoch,
				head: replay.history.head,
				generation: replay.generation,
				reconstructed: false,
			},
		)?;
		self.verified.store(true, Ordering::Release);
		Ok(())
	}
	pub async fn protect(&self) -> Result<()> {
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		if self
			.directory
			.join(STATE)
			.try_exists()
			.map_err(storage_error)?
		{
			self.verify().await?;
			let _guard = self.waiting_lock().await?;
			if self
				.directory
				.join(ACTIVATION)
				.try_exists()
				.map_err(storage_error)?
			{
				let mut ledger = self.load_locked()?;
				let activation: Activation = read(&self.directory.join(ACTIVATION))?;
				let state = self.publication(ledger.epoch)?;
				if activation.epoch != ledger.epoch
					|| state.head.sequence != 0
					|| state.reconstructed
				{
					return Err(Error::Conflict(
						"interrupted protection no longer matches its seed".into(),
					));
				}
				ledger.restoring = activation.was_closed;
				atomic_write(&self.directory.join("ledger.cbor"), &ledger)?;
				self.clear_named(ACTIVATION)?;
			}
			return Ok(());
		}
		let _guard = self.waiting_lock().await?;
		let mut ledger = self.load_units_locked(self.load_locked()?)?;
		let activation = if self
			.directory
			.join(ACTIVATION)
			.try_exists()
			.map_err(storage_error)?
		{
			let saved: Activation = read(&self.directory.join(ACTIVATION))?;
			if saved.epoch != ledger.epoch {
				return Err(Error::Forbidden);
			}
			saved
		} else {
			Activation {
				epoch: ledger.epoch,
				was_closed: ledger.restoring,
			}
		};
		atomic_write(&self.directory.join(ACTIVATION), &activation)?;
		ledger.restoring = true;
		let mut metadata = ledger.clone();
		metadata.units.clear();
		atomic_write(&self.directory.join("ledger.cbor"), &metadata)?;
		atomic_write(&self.directory.join(REQUIRED), &true)?;
		let replay = independent.seed(&ledger).await?;
		let current = independent.replay().await?;
		if current.history.head != replay.history.head {
			return Err(Error::Conflict(
				"memory history already advanced; reconstruct it rather than reseeding".into(),
			));
		}
		atomic_write(
			&self.directory.join(STATE),
			&Publication {
				epoch: ledger.epoch,
				head: replay.history.head,
				generation: replay.generation,
				reconstructed: false,
			},
		)?;
		metadata.restoring = activation.was_closed;
		atomic_write(&self.directory.join("ledger.cbor"), &metadata)?;
		self.clear_named(ACTIVATION)?;
		self.verified.store(true, Ordering::Release);
		Ok(())
	}
	pub(super) async fn publish_locked(&self, metadata: &Ledger, units: &[Unit]) -> Result<()> {
		if !self.protection_requested()? {
			return self.observe_locked(metadata, units);
		}
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		if self
			.directory
			.join(PENDING)
			.try_exists()
			.map_err(storage_error)?
		{
			notify_closed("publication_pending");
			return Err(Error::SemanticUnavailable);
		}
		let mut state = self.publication(metadata.epoch)?;
		// An unavailable preflight has no publication side effect: a proven clean
		// local ledger can still serve reads. After pending is durable, errors close it.
		independent.confirm(&state).await?;
		let mut selected = metadata.clone();
		for unit in units {
			if let Some(fence) = self.unit_locked(metadata, unit.id)? {
				selected.units.insert(unit.id, fence);
			}
		}
		let floors = units
			.iter()
			.map(Floor::from_unit)
			.collect::<aidash_domain::Result<Vec<_>>>()?;
		if floors
			.iter()
			.all(|floor| selected.units.get(&floor.id) == Some(&floor.fence))
		{
			return Ok(());
		}
		let mut history = History {
			ledger: selected,
			head: state.head.clone(),
		};
		let record = history.next(
			Uuid::new_v4(),
			Change::Floors {
				units: floors,
				recovering: state.head.recovering,
			},
		)?;
		history.apply(&record)?;
		let result = async {
			atomic_write(&self.directory.join(PENDING), &record)?;
			let generation = independent.append(&record).await?;
			self.write_fences(&history.ledger)?;
			state.head = history.head;
			state.generation = generation;
			atomic_write(&self.directory.join(STATE), &state)?;
			self.clear_pending()
		}
		.await;
		if result.is_err() {
			self.verified.store(false, Ordering::Release);
			notify_closed("publication_unresolved");
		}
		result
	}
	pub async fn reconstruct(&self) -> Result<Head> {
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		std::fs::create_dir_all(&self.directory).map_err(storage_error)?;
		let _guard = self.waiting_lock().await?;
		if self.directory.join("epoch.cbor").exists() || self.directory.join("ledger.cbor").exists()
		{
			let old: Publication = read(&self.directory.join(STATE))?;
			if !old.reconstructed {
				return Err(Error::Conflict("ledger reconstruction needs a replacement directory or an interrupted reconstruction".into()));
			}
		}
		atomic_write(&self.directory.join(REQUIRED), &true)?;
		let replay = independent.barrier().await?;
		if self.directory.join("epoch.cbor").exists()
			&& read::<Uuid>(&self.directory.join("epoch.cbor"))? != replay.history.ledger.epoch
		{
			return Err(Error::Forbidden);
		}
		let state = Publication {
			epoch: replay.history.ledger.epoch,
			head: replay.history.head.clone(),
			generation: replay.generation,
			reconstructed: true,
		};
		atomic_write(&self.directory.join(STATE), &state)?;
		std::fs::create_dir_all(self.directory.join("units")).map_err(storage_error)?;
		let mut metadata = replay.history.ledger.clone();
		metadata.restoring = true;
		metadata.units.clear();
		atomic_write(&self.directory.join("epoch.cbor"), &metadata.epoch)?;
		atomic_write(&self.directory.join("ledger.cbor"), &metadata)?;
		self.write_fences(&replay.history.ledger)?;
		self.clear_pending()?;
		self.verified.store(true, Ordering::Release);
		Ok(state.head)
	}
	pub async fn resume_reconstructed(&self, expected: &Head, evidence: String) -> Result<()> {
		// A retry first resolves the exact pending record. It may have committed
		// remotely before the prior process lost its response or local disk write.
		self.verify().await?;
		let independent = self
			.independent
			.as_ref()
			.ok_or(Error::SemanticUnavailable)?;
		let _guard = self.waiting_lock().await?;
		let mut ledger = self.load_locked()?;
		let mut state = self.publication(ledger.epoch)?;
		independent.confirm(&state).await?;
		// Only an unfinished reconstruction may reopen the local restore gate.
		if !state.reconstructed || (state.head.recovering && &state.head != expected) {
			return Err(Error::Conflict(
				"memory reconciliation evidence is no longer current".into(),
			));
		}
		if state.head.recovering {
			let mut history = History {
				ledger: ledger.clone(),
				head: state.head.clone(),
			};
			let record = history.next(Uuid::new_v4(), Change::Resume { evidence })?;
			history.apply(&record)?;
			atomic_write(&self.directory.join(PENDING), &record)?;
			state.generation = independent.append(&record).await?;
			state.head = history.head;
		} else {
			// Only the very same head-bound report may finish an interrupted resume.
			let object = independent
				.objects
				.get(
					&independent.record_key(state.head.sequence),
					Some(&state.generation),
				)
				.await?
				.ok_or(Error::SemanticUnavailable)?;
			let record: Record = serde_json::from_slice(&object.bytes)
				.map_err(|_| Error::Invalid("invalid memory recovery history object".into()))?;
			if record.sequence != expected.sequence.checked_add(1).ok_or(Error::Forbidden)?
				|| record.predecessor != expected.digest
				|| !matches!(record.change, Change::Resume { evidence: ref saved } if saved == &evidence)
			{
				return Err(Error::Conflict(
					"memory reconciliation evidence is no longer current".into(),
				));
			}
		}
		// The reconstructed flag closes readers while the local gate is opened.
		atomic_write(&self.directory.join(STATE), &state)?;
		self.clear_pending()?;
		ledger.restoring = false;
		atomic_write(&self.directory.join("ledger.cbor"), &ledger)?;
		state.reconstructed = false;
		atomic_write(&self.directory.join(STATE), &state)?;
		self.verified.store(true, Ordering::Release);
		Ok(())
	}
	pub fn reconstructed(&self) -> Result<bool> {
		let _guard = self.lock()?;
		let ledger = self.load_locked()?;
		Ok(self.protection_requested()? && self.publication(ledger.epoch)?.reconstructed)
	}

	pub fn publication_head(&self) -> Result<Head> {
		let _guard = self.lock()?;
		Ok(self.publication(self.load_locked()?.epoch)?.head)
	}
}

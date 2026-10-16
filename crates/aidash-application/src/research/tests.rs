//! Acceptance cases of the unmerged Web research PR #99, ported to the
//! provider-package contract. Two different search providers run through the
//! same host implementation without any change to it.
use super::*;
use crate::{
	Error,
	ports::research::{EvidenceStore, Reservation, Reserved},
};
use aidash_domain::research::{
	account::AccountProfile,
	citations::EvidenceRef,
	document::{ExtractedPage, Line},
	evidence::{EvidenceFragment, Original, Purge, PurgeReason, RunUsage, SourceRecord},
	revocation,
	search::{ProviderCandidate, ProviderResults, SourceCandidate},
};
use async_trait::async_trait;
use chrono::{DateTime, Duration};
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{
		Mutex,
		atomic::{AtomicBool, AtomicUsize, Ordering},
	},
};

#[derive(Default)]
struct Memory {
	attempts: Mutex<BTreeMap<(Uuid, String), (Reservation, AttemptState)>>,
	sources: Mutex<Vec<SourceRecord>>,
	snapshots: Mutex<Vec<DocumentSnapshot>>,
	/// Retained originals: Run, snapshot and bytes.
	originals: Mutex<Vec<(Uuid, Uuid, Vec<u8>)>>,
	fragments: Mutex<Vec<EvidenceFragment>>,
	spend: Mutex<u64>,
	revoked: Mutex<BTreeSet<Uuid>>,
	/// A revocation that commits just before the next reservation.
	revoke_at_reserve: AtomicBool,
	/// A retention purge that commits just before the next fragment.
	purge_at_fragment: AtomicBool,
	/// A revocation that commits just after the next snapshot read.
	revoke_after_snapshot_read: AtomicBool,
}

impl Memory {
	/// What the Node's store does on an Evidence Revocation.
	fn revoke(&self, run: Uuid) {
		self.revoked.lock().unwrap().insert(run);
		for snapshot in self.snapshots.lock().unwrap().iter_mut() {
			if snapshot.run_id == run {
				purge(snapshot, PurgeReason::Revocation);
			}
		}
		for fragment in self.fragments.lock().unwrap().iter_mut() {
			if fragment.run_id == run {
				fragment.text.clear();
			}
		}
		for ((owner, _), (reservation, state)) in self.attempts.lock().unwrap().iter_mut() {
			if *owner == run
				&& let AttemptState::Settled { result, .. } = state
			{
				*result = revocation::tombstone(reservation.operation);
			}
		}
	}
	/// What the retention sweeper does to one snapshot.
	fn purge_snapshot(&self, id: Uuid) {
		for snapshot in self.snapshots.lock().unwrap().iter_mut() {
			if snapshot.id == id {
				purge(snapshot, PurgeReason::Retention);
			}
		}
	}
	fn is_revoked(&self, run: Uuid) -> bool {
		self.revoked.lock().unwrap().contains(&run)
	}
}

fn purge(snapshot: &mut DocumentSnapshot, reason: PurgeReason) {
	snapshot.lines.clear();
	snapshot.purged.get_or_insert(Purge {
		at: Utc::now(),
		reason,
	});
}

#[async_trait]
impl EvidenceStore for Memory {
	async fn revoked(&self, run: Uuid) -> Result<bool> {
		Ok(self.is_revoked(run))
	}
	async fn attempt(&self, run: Uuid, key: &str) -> Result<Option<AttemptState>> {
		Ok(self
			.attempts
			.lock()
			.unwrap()
			.get(&(run, key.into()))
			.map(|(_, state)| state.clone()))
	}
	async fn usage(&self, run: Uuid) -> Result<RunUsage> {
		let attempts = self.attempts.lock().unwrap();
		let count = |operation| {
			attempts
				.iter()
				.filter(|((owner, _), (reservation, _))| {
					*owner == run && reservation.operation == operation
				})
				.count() as u32
		};
		let snapshots = self.snapshots.lock().unwrap();
		let owned = snapshots
			.iter()
			.filter(|snapshot| snapshot.run_id == run && snapshot.purged.is_none());
		Ok(RunUsage {
			search_attempts: count(Operation::Search),
			fetch_attempts: count(Operation::Fetch),
			sources: self
				.sources
				.lock()
				.unwrap()
				.iter()
				.filter(|source| source.run_id == run)
				.count() as u32,
			snapshots: owned.clone().count() as u32,
			snapshot_bytes: owned
				.flat_map(|snapshot| &snapshot.lines)
				.map(|line| line.text.len() as u64)
				.sum(),
		})
	}
	async fn account_spend(&self, _: &str, _: DateTime<Utc>) -> Result<u64> {
		Ok(*self.spend.lock().unwrap())
	}
	async fn reserve(&self, run: Uuid, reservation: &Reservation) -> Result<Reserved> {
		if self.revoke_at_reserve.swap(false, Ordering::SeqCst) {
			self.revoke(run);
		}
		if self.is_revoked(run) {
			return Ok(Reserved::Revoked);
		}
		let mut attempts = self.attempts.lock().unwrap();
		if attempts.contains_key(&(run, reservation.key.clone())) {
			return Ok(Reserved::Duplicate);
		}
		let held = attempts
			.iter()
			.filter(|((owner, _), (held, _))| {
				*owner == run && held.operation == reservation.operation
			})
			.count() as u32;
		let maximum = match reservation.operation {
			Operation::Search => reservation.limits.search_attempts,
			_ => reservation.limits.fetch_attempts,
		};
		if held >= maximum {
			return Ok(Reserved::AttemptLimit);
		}
		// A dispatched read holds a snapshot slot until it settles.
		if reservation.operation == Operation::Fetch {
			let snapshots = self
				.snapshots
				.lock()
				.unwrap()
				.iter()
				.filter(|snapshot| snapshot.run_id == run && snapshot.purged.is_none())
				.count() as u32;
			let reading = attempts
				.iter()
				.filter(|((owner, _), (held, state))| {
					*owner == run
						&& held.operation == Operation::Fetch
						&& *state == AttemptState::Dispatched
				})
				.count() as u32;
			if snapshots + reading >= reservation.limits.snapshots {
				return Ok(Reserved::SnapshotLimit);
			}
		}
		let mut spend = self.spend.lock().unwrap();
		if let (Some(attribution), Some(budget)) = (&reservation.attribution, &reservation.budget)
			&& *spend + attribution.price_micro_usd > budget.limit_micro_usd
		{
			return Ok(Reserved::BudgetExhausted);
		}
		let mut sources = self.sources.lock().unwrap();
		if reservation.source_url.is_some()
			&& sources.iter().filter(|source| source.run_id == run).count() as u32
				>= reservation.limits.sources
		{
			return Ok(Reserved::SourceLimit);
		}
		if let Some(attribution) = &reservation.attribution {
			*spend += attribution.price_micro_usd;
		}
		attempts.insert(
			(run, reservation.key.clone()),
			(reservation.clone(), AttemptState::Dispatched),
		);
		let source = reservation.source_url.as_ref().map(|url| {
			let record = SourceRecord {
				id: Uuid::new_v4(),
				run_id: run,
				url: url.clone(),
				title: String::new(),
			};
			sources.push(record.clone());
			record.id
		});
		Ok(Reserved::Reserved { source })
	}
	async fn settle(
		&self,
		run: Uuid,
		key: &str,
		outcome: AttemptOutcome,
		result: &Value,
	) -> Result<()> {
		let mut attempts = self.attempts.lock().unwrap();
		match attempts.get_mut(&(run, key.into())) {
			Some(entry) if entry.1 == AttemptState::Dispatched => {
				let result = if self.is_revoked(run) {
					revocation::tombstone(entry.0.operation)
				} else {
					result.clone()
				};
				entry.1 = AttemptState::Settled { outcome, result };
				Ok(())
			}
			_ => Err(crate::Error::Conflict("attempt is not dispatched".into())),
		}
	}
	async fn record_sources(
		&self,
		run: Uuid,
		_: &str,
		sources: &[SourceCandidate],
		max_sources: u32,
	) -> Result<std::result::Result<Vec<SourceRecord>, Failure>> {
		if self.is_revoked(run) {
			return Ok(Err(Failure::EvidenceRevoked));
		}
		let held = self
			.sources
			.lock()
			.unwrap()
			.iter()
			.filter(|source| source.run_id == run)
			.count();
		let room = (max_sources as usize).saturating_sub(held);
		let records = sources
			.iter()
			.take(room)
			.map(|source| SourceRecord {
				id: Uuid::new_v4(),
				run_id: run,
				url: source.url.clone(),
				title: source.title.clone(),
			})
			.collect::<Vec<_>>();
		self.sources.lock().unwrap().extend(records.clone());
		Ok(Ok(records))
	}
	async fn source(&self, run: Uuid, id: Uuid) -> Result<Option<SourceRecord>> {
		Ok(self
			.sources
			.lock()
			.unwrap()
			.iter()
			.find(|source| source.id == id && source.run_id == run)
			.cloned())
	}
	async fn record_snapshot(
		&self,
		run: Uuid,
		source_id: Uuid,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>> {
		if self.is_revoked(run) {
			return Ok(Err(Failure::EvidenceRevoked));
		}
		let usage = self.usage(run).await?;
		let bytes = page
			.lines
			.iter()
			.map(|line| line.text.len() as u64)
			.sum::<u64>();
		if usage.snapshots >= limits.snapshots
			|| usage.snapshot_bytes.saturating_add(bytes) > limits.snapshot_bytes
		{
			return Ok(Err(Failure::SnapshotLimit));
		}
		let id = Uuid::new_v4();
		// An original is retained within the Run's limit, as the Node's store does.
		let original = page.original.as_ref().map(|bytes| {
			let mut originals = self.originals.lock().unwrap();
			let held = originals
				.iter()
				.filter(|(owner, _, _)| *owner == run)
				.map(|(_, _, held)| held.len() as u64)
				.sum::<u64>();
			let retained = held + bytes.len() as u64 <= limits.original_bytes;
			if retained {
				originals.push((run, id, bytes.clone()));
			}
			Original {
				digest: format!("sha256:{}", bytes.len()),
				bytes: bytes.len() as u64,
				retained,
			}
		});
		let snapshot = DocumentSnapshot {
			id,
			run_id: run,
			source_id,
			requested_url: requested_url.into(),
			url: page.url.clone(),
			title: page.title.clone(),
			media_type: page.media_type.clone(),
			fetched_at: page.fetched_at,
			raw_digest: page.raw_digest.clone(),
			text_digest: document::text_digest(&page.lines),
			completeness: page.completeness,
			lines: page.lines.clone(),
			page_count: page.page_count,
			pages_without_text: page.pages_without_text.clone(),
			original,
			purged: None,
		};
		self.snapshots.lock().unwrap().push(snapshot.clone());
		Ok(Ok(snapshot))
	}
	async fn original(&self, run: Uuid, snapshot: Uuid) -> Result<Option<Vec<u8>>> {
		Ok(self
			.originals
			.lock()
			.unwrap()
			.iter()
			.find(|(owner, id, _)| *owner == run && *id == snapshot)
			.map(|(_, _, bytes)| bytes.clone()))
	}
	async fn snapshot(&self, run: Uuid, id: Uuid) -> Result<Option<DocumentSnapshot>> {
		let snapshot = self
			.snapshots
			.lock()
			.unwrap()
			.iter()
			.find(|snapshot| snapshot.id == id && snapshot.run_id == run)
			.cloned();
		if self
			.revoke_after_snapshot_read
			.swap(false, Ordering::SeqCst)
		{
			self.revoke(run);
		}
		Ok(snapshot)
	}
	async fn record_fragment(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		passage: &Passage,
	) -> Result<std::result::Result<EvidenceFragment, Failure>> {
		if self.is_revoked(run) {
			return Ok(Err(Failure::EvidenceRevoked));
		}
		if self.purge_at_fragment.swap(false, Ordering::SeqCst) {
			self.purge_snapshot(document.id);
		}
		// The stored snapshot, not the caller's earlier copy, decides.
		let stored = self
			.snapshots
			.lock()
			.unwrap()
			.iter()
			.find(|snapshot| snapshot.id == document.id && snapshot.run_id == run)
			.map(DocumentSnapshot::readable);
		match stored {
			None => return Ok(Err(Failure::UnknownDocument)),
			Some(Err(unreadable)) => return Ok(Err(unreadable)),
			Some(Ok(())) => {}
		}
		let fragment = EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), document, passage);
		self.fragments.lock().unwrap().push(fragment.clone());
		Ok(Ok(fragment))
	}
	async fn fragments(
		&self,
		run: Uuid,
		references: &[EvidenceRef],
	) -> Result<Vec<EvidenceFragment>> {
		if self.is_revoked(run) {
			return Ok(vec![]);
		}
		Ok(self
			.fragments
			.lock()
			.unwrap()
			.iter()
			.filter(|fragment| fragment.run_id == run && references.contains(&fragment.reference))
			.cloned()
			.collect())
	}
}

/// Approves only digests explicitly approved by a person; anything else waits.
#[derive(Default)]
struct Gate {
	/// Approvals by ID and the digest each one approves.
	approved: Mutex<BTreeMap<Uuid, String>>,
	denied: Mutex<Vec<String>>,
	requests: Mutex<Vec<WebDisclosure>>,
	permit_all: bool,
}

impl Gate {
	fn permitting() -> Self {
		Self {
			permit_all: true,
			..Self::default()
		}
	}
	fn decide(&self, run: Uuid, allow: bool) {
		let last = self.requests.lock().unwrap().last().cloned().unwrap();
		if allow {
			self.approved
				.lock()
				.unwrap()
				.insert(Uuid::new_v4(), last.digest(run));
		} else {
			self.denied.lock().unwrap().push(last.digest(run));
		}
	}
}

#[async_trait]
impl DisclosureGate for Gate {
	async fn admit(&self, run: Uuid, disclosure: &WebDisclosure) -> Result<DisclosureState> {
		self.requests.lock().unwrap().push(disclosure.clone());
		let digest = disclosure.digest(run);
		Ok(if self.permit_all {
			DisclosureState::Permitted
		} else if self.denied.lock().unwrap().contains(&digest) {
			DisclosureState::Denied
		} else if let Some(approval_id) = self
			.approved
			.lock()
			.unwrap()
			.iter()
			.find_map(|(id, approved)| (*approved == digest).then_some(*id))
		{
			DisclosureState::Approved { approval_id }
		} else {
			DisclosureState::Pending {
				approval_id: Uuid::from_u128(77),
				expires_at: Some(Utc::now() + Duration::minutes(10)),
			}
		})
	}
	async fn consume(&self, _: Uuid, approval_id: Uuid) -> Result<bool> {
		// An exact approval is consumed by the one disclosure it allows.
		Ok(self.approved.lock().unwrap().remove(&approval_id).is_some())
	}
}

/// A search provider whose candidates name it, so a swap is observable.
struct Search {
	name: &'static str,
	destination: &'static str,
	calls: AtomicUsize,
	outcome: Option<Failure>,
	account: Option<AccountProfile>,
}

impl Search {
	fn new(name: &'static str, destination: &'static str) -> Self {
		Self {
			name,
			destination,
			calls: AtomicUsize::new(0),
			outcome: None,
			account: None,
		}
	}
}

#[async_trait]
impl SearchProvider for Search {
	fn disclosure(&self, query: &SearchQuery) -> std::result::Result<Outbound, Failure> {
		Ok(Outbound {
			destination: self.destination.into(),
			content: query.query.clone(),
		})
	}
	fn account(&self) -> std::result::Result<Option<AccountProfile>, Failure> {
		Ok(self.account.clone())
	}
	async fn search(&self, query: &SearchQuery) -> std::result::Result<ProviderResults, Failure> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		if let Some(failure) = self.outcome {
			return Err(failure);
		}
		Ok(ProviderResults {
			candidates: vec![
				ProviderCandidate {
					url: format!("https://{}.example.com/{}", self.name, query.query),
					title: format!("{} result", self.name),
					snippet: Some("discovery hint".into()),
					page_age: None,
				},
				ProviderCandidate {
					// Credentials in a URL are never a Source record.
					url: "https://user:secret@example.com/admin".into(),
					title: "Unfiltered".into(),
					snippet: None,
					page_age: None,
				},
			],
			more_results_available: false,
		})
	}
}

struct Pages {
	calls: AtomicUsize,
	lines: Vec<Line>,
}

impl Pages {
	fn new() -> Self {
		Self {
			calls: AtomicUsize::new(0),
			lines: vec![
				Line {
					text: "Alpha line".into(),
					page: None,
				},
				Line {
					text: "Die Straße <script>inert text</script>".into(),
					page: Some(2),
				},
				Line {
					text: "Omega line".into(),
					page: Some(2),
				},
			],
		}
	}
}

#[async_trait]
impl PageProvider for Pages {
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure> {
		match url.host_str() {
			Some("localhost" | "127.0.0.1") => Err(Failure::NonPublicDestination),
			_ => Ok(()),
		}
	}
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		if url.host_str() == Some("private.example.com") {
			// Resolves to a private address at dispatch.
			return Err(Failure::NonPublicDestination);
		}
		Ok(ExtractedPage {
			url: url.to_string(),
			title: Some("Source".into()),
			media_type: "text/plain".into(),
			// A faulty provider dates its page after the retrieval.
			fetched_at: if url.host_str() == Some("future.example.com") {
				Utc::now() + Duration::days(1)
			} else {
				Utc::now()
			},
			raw_digest: "sha256:raw".into(),
			completeness: Completeness::Complete,
			lines: self.lines.clone(),
			page_count: None,
			pages_without_text: Vec::new(),
			original: None,
		})
	}
}

/// A PDF with `pages` pages whose listed pages carry no text, as page reading
/// returns it: the original travels with the page.
struct ScannedPdf {
	lines: Vec<Line>,
	pages_without_text: Vec<u32>,
	original_bytes: usize,
}

#[async_trait]
impl PageProvider for ScannedPdf {
	fn admit(&self, _: &Url) -> std::result::Result<(), Failure> {
		Ok(())
	}
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure> {
		Ok(ExtractedPage {
			url: url.to_string(),
			title: Some("Scanned report".into()),
			media_type: "application/pdf".into(),
			fetched_at: Utc::now(),
			raw_digest: "sha256:pdf".into(),
			completeness: Completeness::Complete,
			lines: self.lines.clone(),
			page_count: Some(3),
			pages_without_text: self.pages_without_text.clone(),
			original: Some(vec![b'%'; self.original_bytes]),
		})
	}
}

fn call<'a>(run: Uuid, key: &'a str, provider: &'a str) -> Call<'a> {
	Call {
		run,
		context_revision: 0,
		key,
		provider,
		disclosure_approval: true,
	}
}

#[tokio::test]
async fn a_second_search_provider_replaces_the_first_through_the_same_host_contract() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	for (key, provider, name) in [
		(
			"k1",
			Search::new("brave", "api.search.brave.com"),
			"research.brave@1",
		),
		(
			"k2",
			Search::new("fixture", "fixture.invalid"),
			"research.fixture@1",
		),
	] {
		let result = research
			.search(&call(run, key, name), &provider, &json!({"query":"rust"}))
			.await
			.unwrap();
		assert_eq!(result["status"], "ok", "{result}");
		assert_eq!(result["data"]["provider"], name);
		assert_eq!(result["data"]["returned_count"], 1);
		assert_eq!(
			result["data"]["filtered_count"], 1,
			"the host filters every provider"
		);
		let source = &result["data"]["sources"][0];
		assert_eq!(source["evidence_state"], "unread");
		let id: Uuid = serde_json::from_value(source["source_id"].clone()).unwrap();
		assert!(store.source(run, id).await.unwrap().is_some());
		assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
	}
	let destinations = gate
		.requests
		.lock()
		.unwrap()
		.iter()
		.map(|request| request.outbound.destination.clone())
		.collect::<Vec<_>>();
	assert_eq!(destinations, ["api.search.brave.com", "fixture.invalid"]);
}

#[tokio::test]
async fn disclosure_waits_then_an_exact_approval_dispatches_once_and_denial_never_dispatches() {
	let (store, gate) = (Memory::default(), Gate::default());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let input = json!({"url":"https://example.com/page"});
	let waiting = research
		.fetch(&call(run, "k1", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(waiting["status"], "approval_required");
	assert_eq!(waiting["approval_id"], json!(Uuid::from_u128(77)));
	assert_eq!(waiting["data"]["content"], "https://example.com/page");
	assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
	assert_eq!(store.usage(run).await.unwrap().fetch_attempts, 0);

	gate.decide(run, true);
	let opened = research
		.fetch(&call(run, "k2", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(opened["status"], "ok", "{opened}");
	assert_eq!(pages.calls.load(Ordering::SeqCst), 1);
	// The approval was consumed by the call it allowed.
	let again = research
		.fetch(&call(run, "k3", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(again["status"], "approval_required");

	gate.decide(run, false);
	let denied = research
		.fetch(&call(run, "k4", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(denied["error"]["code"], "disclosure_denied");
	assert_eq!(pages.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn new_user_input_needs_a_new_approval() {
	let (store, gate) = (Memory::default(), Gate::default());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	let input = json!({"query":"rust"});
	research
		.search(&call(run, "k1", "research.brave@1"), &provider, &input)
		.await
		.unwrap();
	gate.decide(run, true);
	let mut later = call(run, "k2", "research.brave@1");
	later.context_revision = 1;
	let result = research.search(&later, &provider, &input).await.unwrap();
	assert_eq!(result["status"], "approval_required");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_cannot_override_non_public_destinations() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let literal = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"http://localhost/"}),
		)
		.await
		.unwrap();
	assert_eq!(literal["error"]["code"], "non_public_destination");
	assert!(
		gate.requests.lock().unwrap().is_empty(),
		"rejected before disclosure"
	);
	assert_eq!(store.usage(run).await.unwrap().fetch_attempts, 0);
	let resolved = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&pages,
			&json!({"url":"https://private.example.com/"}),
		)
		.await
		.unwrap();
	assert_eq!(resolved["error"]["code"], "non_public_destination");
	assert!(store.snapshots.lock().unwrap().is_empty());
}

#[tokio::test]
async fn run_limits_block_before_disclosure_or_dispatch() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			search_attempts: 1,
			..Limits::default()
		},
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	let blocked = research
		.search(
			&call(run, "k2", "research.brave@1"),
			&provider,
			&json!({"query":"b"}),
		)
		.await
		.unwrap();
	assert_eq!(blocked["error"]["code"], "attempt_limit");
	assert_eq!(gate.requests.lock().unwrap().len(), 1);
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn direct_url_fetches_count_against_the_source_limit() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			sources: 1,
			..Limits::default()
		},
	};
	let (provider, pages) = (Search::new("brave", "api.search.brave.com"), Pages::new());
	let run = Uuid::new_v4();
	let searched = research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	let source = searched["data"]["sources"][0]["source_id"].clone();
	// Search filled the Run's Source records; a new address is refused
	// before disclosure or dispatch.
	let blocked = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/elsewhere"}),
		)
		.await
		.unwrap();
	assert_eq!(blocked["error"]["code"], "source_limit");
	assert_eq!(gate.requests.lock().unwrap().len(), 1);
	assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
	// Reading an existing Source record creates none.
	let read = research
		.fetch(
			&call(run, "k3", "research.page@1"),
			&pages,
			&json!({"source_id":source}),
		)
		.await
		.unwrap();
	assert_eq!(read["status"], "ok", "{read}");
	assert_eq!(store.sources.lock().unwrap().len(), 1);
}

/// Records a Source while the disclosure is being admitted, as a concurrent
/// search of the same Run would.
struct ConcurrentSource<'a> {
	store: &'a Memory,
}

#[async_trait]
impl DisclosureGate for ConcurrentSource<'_> {
	async fn admit(&self, run: Uuid, _: &WebDisclosure) -> Result<DisclosureState> {
		let candidate = SourceCandidate {
			url: "https://example.com/concurrent".into(),
			title: "Concurrent".into(),
			snippet: String::new(),
			rank: 1,
			page_age: None,
			truncated: false,
		};
		self.store
			.record_sources(run, "concurrent", &[candidate], 1)
			.await?
			.expect("the Run is not revoked");
		Ok(DisclosureState::Permitted)
	}
	async fn consume(&self, _: Uuid, _: Uuid) -> Result<bool> {
		Ok(true)
	}
}

#[tokio::test]
async fn a_concurrent_source_cannot_take_the_slot_of_a_page_read_by_address() {
	let store = Memory::default();
	let gate = ConcurrentSource { store: &store };
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			sources: 1,
			..Limits::default()
		},
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let result = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source"}),
		)
		.await
		.unwrap();
	// The reservation finds the slot taken: nothing is sent or reserved.
	assert_eq!(result["error"]["code"], "source_limit");
	assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
	assert_eq!(store.attempt(run, "k1").await.unwrap(), None);
	assert_eq!(store.sources.lock().unwrap().len(), 1);
	assert!(store.snapshots.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_page_read_by_address_holds_its_source_slot_from_the_reservation() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			sources: 1,
			..Limits::default()
		},
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let read = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source"}),
		)
		.await
		.unwrap();
	assert_eq!(read["status"], "ok", "{read}");
	let sources = store.sources.lock().unwrap().clone();
	assert_eq!(sources.len(), 1);
	assert_eq!(sources[0].url, "https://example.com/source");
	assert_eq!(store.snapshots.lock().unwrap()[0].source_id, sources[0].id);
	// A search reserved later finds no room for its candidates.
	let candidate = SourceCandidate {
		url: "https://example.com/later".into(),
		title: "Later".into(),
		snippet: String::new(),
		rank: 1,
		page_age: None,
		truncated: false,
	};
	assert!(
		store
			.record_sources(run, "later", &[candidate], 1)
			.await
			.unwrap()
			.unwrap()
			.is_empty()
	);
}

#[tokio::test]
async fn costs_are_attributed_to_the_search_service_account_before_dispatch() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let now = Utc::now();
	let mut provider = Search::new("brave", "api.search.brave.com");
	provider.account = Some(AccountProfile {
		account_id: "deployment-administrator-account".into(),
		agreement_reference: "contract".into(),
		verified_by: "deployment_administrator".into(),
		verified_at: now - Duration::days(1),
		review_after: now + Duration::days(1),
		storage_permitted: true,
		query_training_prohibited: true,
		evaluation_permitted: true,
		retention_days: 30,
		deletion_after_termination_days: 30,
		credential_env: "AIDASH_SECRET_SEARCH".into(),
		price_effective_at: now - Duration::days(1),
		price_review_after: now + Duration::days(1),
		request_price_micro_usd: 5_000,
		monthly_base_fee_micro_usd: 0,
		monthly_limit_micro_usd: 5_000,
	});
	let run = Uuid::new_v4();
	let first = research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	assert_eq!(first["status"], "ok");
	let attempts = store.attempts.lock().unwrap().clone();
	let (reservation, _) = attempts.values().next().unwrap();
	assert_eq!(
		reservation.attribution.as_ref().unwrap().account_id,
		"deployment-administrator-account"
	);
	let exhausted = research
		.search(
			&call(run, "k2", "research.brave@1"),
			&provider,
			&json!({"query":"b"}),
		)
		.await
		.unwrap();
	assert_eq!(exhausted["error"]["code"], "budget_exhausted");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn recorded_attempts_replay_with_the_same_evidence_and_unknown_dispatch_is_never_repeated() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	let input = json!({"query":"rust"});
	let first = research
		.search(&call(run, "k1", "research.brave@1"), &provider, &input)
		.await
		.unwrap();
	let replay = research
		.search(&call(run, "k1", "research.brave@1"), &provider, &input)
		.await
		.unwrap();
	assert_eq!(
		first, replay,
		"the recorded result keeps its Source record identity"
	);
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

	// A crash after dispatch leaves an attempt without an outcome.
	store
		.reserve(
			run,
			&Reservation {
				key: "k2".into(),
				operation: Operation::Search,
				provider: "research.brave@1".into(),
				destination: "api.search.brave.com".into(),
				attribution: None,
				limits: Limits::default(),
				budget: None,
				source_url: None,
			},
		)
		.await
		.unwrap();
	let unknown = research
		.search(&call(run, "k2", "research.brave@1"), &provider, &input)
		.await
		.unwrap();
	assert_eq!(unknown["status"], "uncertain");
	assert_eq!(unknown["error"]["code"], "outcome_unknown");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

	let mut failing = Search::new("brave", "api.search.brave.com");
	failing.outcome = Some(Failure::Timeout);
	let timeout = research
		.search(&call(run, "k3", "research.brave@1"), &failing, &input)
		.await
		.unwrap();
	assert_eq!(timeout["error"]["code"], "timeout");
	assert!(matches!(
		store.attempt(run, "k3").await.unwrap(),
		Some(AttemptState::Settled {
			outcome: AttemptOutcome::Unknown,
			..
		})
	));
}

#[tokio::test]
async fn snapshot_continuation_unicode_find_and_fragments_stay_run_scoped() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let first = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source","max_bytes":5}),
		)
		.await
		.unwrap();
	assert_eq!(first["status"], "partial");
	assert_eq!(first["data"]["text"], "Alpha");
	let document = first["data"]["document_id"].clone();
	let cursor = first["data"]["next_cursor"].clone();
	assert!(cursor.is_string());

	let next = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&pages,
			&json!({"document_id":document,"cursor":cursor,"max_bytes":24}),
		)
		.await
		.unwrap();
	assert!(
		next["data"]["text"]
			.as_str()
			.unwrap()
			.starts_with(" line\nDie Straße"),
		"{next}"
	);
	assert_eq!(
		pages.calls.load(Ordering::SeqCst),
		1,
		"continuation never refetches"
	);

	let found = research
		.find(
			&call(run, "k3", "research.page@1"),
			&json!({"document_id":document,"query":"STRASSE"}),
		)
		.await
		.unwrap();
	assert_eq!(found["status"], "ok", "{found}");
	let hit = &found["data"]["matches"][0];
	assert_eq!(hit["line_start"], 2);
	assert_eq!(hit["pages"], json!([2]));
	assert!(hit["text"].as_str().unwrap().contains("Straße"));
	let reference: EvidenceRef = hit["evidence_ref"].as_str().unwrap().parse().unwrap();
	assert_eq!(store.fragments(run, &[reference]).await.unwrap().len(), 1);

	// Another Run cannot address this snapshot, its fragments or its cursor.
	let foreign = Uuid::new_v4();
	assert!(
		store
			.fragments(foreign, &[reference])
			.await
			.unwrap()
			.is_empty()
	);
	let forged = research
		.fetch(
			&call(foreign, "k4", "research.page@1"),
			&pages,
			&json!({"document_id":document,"cursor":cursor}),
		)
		.await
		.unwrap();
	assert_eq!(forged["error"]["code"], "unknown_document");
	let wrong_cursor = research
		.find(
			&call(run, "k5", "research.page@1"),
			&json!({"document_id":document,"query":"line","cursor":cursor}),
		)
		.await
		.unwrap();
	assert_eq!(wrong_cursor["error"]["code"], "invalid_cursor");
	let unknown_source = research
		.fetch(
			&call(run, "k6", "research.page@1"),
			&pages,
			&json!({"source_id":Uuid::new_v4()}),
		)
		.await
		.unwrap();
	assert_eq!(unknown_source["error"]["code"], "unknown_source");
	assert_eq!(pages.calls.load(Ordering::SeqCst), 1, "no hidden fetch");
}

#[tokio::test]
async fn find_pages_matches_with_a_query_bound_cursor() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	let opened = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source"}),
		)
		.await
		.unwrap();
	let document = opened["data"]["document_id"].clone();
	let first = research
		.find(
			&call(run, "k2", "research.page@1"),
			&json!({"document_id":document,"query":"line","max_matches":1}),
		)
		.await
		.unwrap();
	assert_eq!(first["status"], "partial");
	let rest = research
		.find(
			&call(run, "k3", "research.page@1"),
			&json!({"document_id":document,"query":"line","max_matches":1,"cursor":first["data"]["next_cursor"]}),
		)
		.await
		.unwrap();
	assert_eq!(rest["status"], "ok");
	assert_eq!(rest["data"]["matches"][0]["line_start"], 3);
	let none = research
		.find(
			&call(run, "k4", "research.page@1"),
			&json!({"document_id":document,"query":"absent"}),
		)
		.await
		.unwrap();
	assert_eq!(none["status"], "empty");
}

#[tokio::test]
async fn delivered_results_fit_the_envelope_without_hidden_loss() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let mut pages = Pages::new();
	// Quotes double in JSON; the delivery shrinks instead of truncating silently.
	pages.lines = vec![
		Line {
			text: "\"".repeat(16 * 1024),
			page: None,
		};
		3
	];
	let run = Uuid::new_v4();
	let result = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/quotes","max_bytes":16384}),
		)
		.await
		.unwrap();
	assert!(result.to_string().len() <= MAX_RESULT_BYTES);
	assert_eq!(result["status"], "partial");
	let delivered = result["data"]["text"].as_str().unwrap().len();
	assert!(delivered > 0 && delivered < 16 * 1024);
	let next = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&pages,
			&json!({"document_id":result["data"]["document_id"],"cursor":result["data"]["next_cursor"],"max_bytes":16384}),
		)
		.await
		.unwrap();
	assert_eq!(
		next["data"]["line_start"], 1,
		"the next delivery resumes exactly where the previous stopped"
	);
}

#[tokio::test]
async fn a_byte_budget_smaller_than_the_next_character_still_delivers_it() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let mut pages = Pages::new();
	pages.lines[0].text = "ßeta line".into();
	let run = Uuid::new_v4();
	let first = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/sharp","max_bytes":1}),
		)
		.await
		.unwrap();
	assert_eq!(first["status"], "partial", "{first}");
	assert_eq!(first["data"]["text"], "ß");
	let next = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&pages,
			&json!({"document_id":first["data"]["document_id"],"cursor":first["data"]["next_cursor"],"max_bytes":3}),
		)
		.await
		.unwrap();
	assert_eq!(next["data"]["text"], "eta", "{next}");
}

#[tokio::test]
async fn invalid_input_never_reaches_disclosure() {
	let (store, gate) = (Memory::default(), Gate::default());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Pages::new();
	let run = Uuid::new_v4();
	for input in [
		json!({}),
		json!({"url":"https://example.com/","source_id":Uuid::new_v4()}),
		json!({"url":"ftp://example.com/"}),
		json!({"url":"https://example.com/","cursor":"c1.x"}),
		json!({"url":"https://example.com/","max_bytes":0}),
	] {
		let result = research
			.fetch(&call(run, "k", "research.page@1"), &pages, &input)
			.await
			.unwrap();
		assert_eq!(result["error"]["code"], "invalid_input", "{input}");
	}
	assert!(gate.requests.lock().unwrap().is_empty());
}

/// Reserves another attempt while the disclosure is being admitted, as a
/// concurrent invocation of the same Run would.
struct Concurrent<'a> {
	store: &'a Memory,
}

#[async_trait]
impl DisclosureGate for Concurrent<'_> {
	async fn admit(&self, run: Uuid, _: &WebDisclosure) -> Result<DisclosureState> {
		self.store
			.reserve(
				run,
				&Reservation {
					key: "concurrent".into(),
					operation: Operation::Search,
					provider: "research.brave@1".into(),
					destination: "api.search.brave.com".into(),
					attribution: None,
					limits: Limits {
						search_attempts: 1,
						..Limits::default()
					},
					budget: None,
					source_url: None,
				},
			)
			.await?;
		Ok(DisclosureState::Permitted)
	}
	async fn consume(&self, _: Uuid, _: Uuid) -> Result<bool> {
		Ok(true)
	}
}

#[tokio::test]
async fn a_concurrent_attempt_cannot_exceed_the_run_limit_after_the_early_check() {
	let store = Memory::default();
	let gate = Concurrent { store: &store };
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			search_attempts: 1,
			..Limits::default()
		},
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	let result = research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	assert_eq!(result["error"]["code"], "attempt_limit");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
	assert_eq!(store.attempt(run, "k1").await.unwrap(), None);
}

/// Approves once, then lets a concurrent invocation take the Run's last
/// attempt before the reservation.
struct RacedApproval<'a> {
	store: &'a Memory,
	consumed: AtomicUsize,
}

#[async_trait]
impl DisclosureGate for RacedApproval<'_> {
	async fn admit(&self, run: Uuid, _: &WebDisclosure) -> Result<DisclosureState> {
		Concurrent { store: self.store }
			.admit(
				run,
				&WebDisclosure {
					operation: Operation::Search,
					provider: "research.brave@1".into(),
					outbound: Outbound {
						destination: "api.search.brave.com".into(),
						content: "other".into(),
					},
					context_revision: 0,
				},
			)
			.await?;
		Ok(DisclosureState::Approved {
			approval_id: Uuid::from_u128(5),
		})
	}
	async fn consume(&self, _: Uuid, _: Uuid) -> Result<bool> {
		self.consumed.fetch_add(1, Ordering::SeqCst);
		Ok(true)
	}
}

#[tokio::test]
async fn an_approval_is_consumed_only_after_the_attempt_is_reserved() {
	let store = Memory::default();
	let gate = RacedApproval {
		store: &store,
		consumed: AtomicUsize::new(0),
	};
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			search_attempts: 1,
			..Limits::default()
		},
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	let blocked = research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	assert_eq!(blocked["error"]["code"], "attempt_limit");
	assert_eq!(
		gate.consumed.load(Ordering::SeqCst),
		0,
		"the approval stays usable"
	);

	// With room, the reservation succeeds and then consumes the approval.
	let roomy = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let result = roomy
		.search(
			&call(run, "k2", "research.brave@1"),
			&provider,
			&json!({"query":"b"}),
		)
		.await
		.unwrap();
	assert_eq!(result["status"], "ok", "{result}");
	assert_eq!(gate.consumed.load(Ordering::SeqCst), 1);
}

/// An approval another invocation already consumed never dispatches.
struct Spent;
#[async_trait]
impl DisclosureGate for Spent {
	async fn admit(&self, _: Uuid, _: &WebDisclosure) -> Result<DisclosureState> {
		Ok(DisclosureState::Approved {
			approval_id: Uuid::from_u128(6),
		})
	}
	async fn consume(&self, _: Uuid, _: Uuid) -> Result<bool> {
		Ok(false)
	}
}

#[tokio::test]
async fn a_spent_approval_settles_the_reserved_attempt_without_dispatch() {
	let store = Memory::default();
	let research = Research {
		store: &store,
		gate: &Spent,
		limits: Limits::default(),
	};
	let provider = Search::new("brave", "api.search.brave.com");
	let run = Uuid::new_v4();
	let result = research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	assert_eq!(result["error"]["code"], "disclosure_denied");
	assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
	assert!(matches!(
		store.attempt(run, "k1").await.unwrap(),
		Some(AttemptState::Settled {
			outcome: AttemptOutcome::Failed,
			..
		})
	));
}

/// A provider failure after the call stays attributed; only an attempt
/// stopped before the call (above) is not.
#[rstest::rstest]
#[case::rejected(Failure::ProviderRequestRejected, AttemptOutcome::Completed)]
#[case::invalid(Failure::ProviderResponseInvalid, AttemptOutcome::Completed)]
#[case::too_large(Failure::ResponseTooLarge, AttemptOutcome::Completed)]
#[case::timeout(Failure::Timeout, AttemptOutcome::Unknown)]
#[tokio::test]
async fn provider_failures_after_the_call_stay_charged(
	#[case] error: Failure,
	#[case] expected: AttemptOutcome,
) {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let mut provider = Search::new("brave", "api.search.brave.com");
	provider.outcome = Some(error);
	let run = Uuid::new_v4();
	research
		.search(
			&call(run, "k1", "research.brave@1"),
			&provider,
			&json!({"query":"a"}),
		)
		.await
		.unwrap();
	let Some(AttemptState::Settled { outcome, .. }) = store.attempt(run, "k1").await.unwrap()
	else {
		panic!("settled");
	};
	assert_eq!(outcome, expected);
	assert!(outcome.charged());
}

/// A page provider whose every read ends with one recorded failure.
struct Failing {
	failure: Failure,
	calls: AtomicUsize,
}

#[async_trait]
impl PageProvider for Failing {
	fn admit(&self, _: &Url) -> std::result::Result<(), Failure> {
		Ok(())
	}
	async fn fetch(&self, _: &Url) -> std::result::Result<ExtractedPage, Failure> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		Err(self.failure)
	}
}

/// Ported from PR #99 `recovered_redirect_and_long_retry_receipts_do_not_repeat_the_old_request`:
/// a recorded cross-host redirect or rate-limit outcome is final for its
/// invocation and is replayed, never sent again. Another host needs its own call
/// and its own Web disclosure approval.
#[rstest::rstest]
#[case::redirect(Failure::CrossHostRedirect, "cross_host_redirect")]
#[case::long_retry(Failure::RateLimited, "rate_limited")]
#[tokio::test]
async fn recorded_redirect_and_retry_outcomes_never_repeat_the_old_request(
	#[case] outcome: Failure,
	#[case] code: &str,
) {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let pages = Failing {
		failure: outcome,
		calls: AtomicUsize::new(0),
	};
	let run = Uuid::new_v4();
	let input = json!({"url":"https://example.com/"});
	let first = research
		.fetch(&call(run, "k1", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(first["error"]["code"], code);
	let replay = research
		.fetch(&call(run, "k1", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(replay, first);
	assert_eq!(
		pages.calls.load(Ordering::SeqCst),
		1,
		"the old request is not repeated"
	);
	assert_eq!(
		gate.requests.lock().unwrap().len(),
		1,
		"no second disclosure"
	);
}

#[tokio::test]
async fn a_revoked_run_does_no_further_research_and_never_replays_a_recorded_result() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let (run, search, pages) = (
		Uuid::new_v4(),
		Search::new("brave", "api.search.brave.com"),
		Pages::new(),
	);
	let input = json!({"url": "https://example.com/page"});
	let read = research
		.fetch(&call(run, "read", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(read["status"], "ok", "{read}");
	let reference: EvidenceRef =
		serde_json::from_value(read["data"]["evidence_ref"].clone()).unwrap();
	let document = read["data"]["document_id"].clone();
	store.revoke(run);

	// The same invocation key finds a tombstone, never the delivered passage.
	let replayed = research
		.fetch(&call(run, "read", "research.page@1"), &pages, &input)
		.await
		.unwrap();
	assert_eq!(replayed["error"]["code"], "evidence_revoked", "{replayed}");
	for result in [
		research
			.search(
				&call(run, "search", "research.brave@1"),
				&search,
				&json!({"query": "rust"}),
			)
			.await
			.unwrap(),
		research
			.fetch(
				&call(run, "again", "research.page@1"),
				&pages,
				&json!({"document_id": document}),
			)
			.await
			.unwrap(),
		research
			.find(
				&call(run, "find", "research.page@1"),
				&json!({"document_id": document, "query": "line"}),
			)
			.await
			.unwrap(),
	] {
		assert_eq!(result["error"]["code"], "evidence_revoked", "{result}");
		assert!(result.get("data").is_none());
	}
	assert_eq!(search.calls.load(Ordering::SeqCst), 0);
	assert_eq!(pages.calls.load(Ordering::SeqCst), 1);
	// Later citations of the Run's evidence are unknown.
	assert!(store.fragments(run, &[reference]).await.unwrap().is_empty());
}

/// Revokes the Run while the page is being read, after the attempt was
/// reserved and the provider called.
struct RevokingPages<'a> {
	store: &'a Memory,
	run: Uuid,
	pages: Pages,
}

#[async_trait]
impl PageProvider for RevokingPages<'_> {
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure> {
		self.pages.admit(url)
	}
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure> {
		self.store.revoke(self.run);
		self.pages.fetch(url).await
	}
}

#[tokio::test]
async fn an_attempt_in_flight_at_revocation_records_nothing_and_stays_charged() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	let pages = RevokingPages {
		store: &store,
		run,
		pages: Pages::new(),
	};
	let result = research
		.fetch(
			&call(run, "read", "research.page@1"),
			&pages,
			&json!({"url": "https://example.com/page"}),
		)
		.await
		.unwrap();
	assert_eq!(result["error"]["code"], "evidence_revoked", "{result}");
	assert!(store.snapshots.lock().unwrap().is_empty());
	assert!(store.fragments.lock().unwrap().is_empty());
	// The provider was called, so the attempt is settled, not dispatched.
	assert_eq!(
		store.attempt(run, "read").await.unwrap(),
		Some(AttemptState::Settled {
			outcome: AttemptOutcome::Completed,
			result: revocation::tombstone(Operation::Fetch),
		})
	);
	assert_eq!(store.usage(run).await.unwrap().fetch_attempts, 1);
}

#[tokio::test]
async fn a_purged_snapshot_is_distinguishable_and_its_source_can_be_read_again() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let (run, pages) = (Uuid::new_v4(), Pages::new());
	let read = research
		.fetch(
			&call(run, "read", "research.page@1"),
			&pages,
			&json!({"url": "https://example.com/page", "max_bytes": 12}),
		)
		.await
		.unwrap();
	let cursor = read["data"]["next_cursor"].clone();
	assert!(cursor.is_string(), "{read}");
	let document: Uuid = serde_json::from_value(read["data"]["document_id"].clone()).unwrap();
	let reference: EvidenceRef =
		serde_json::from_value(read["data"]["evidence_ref"].clone()).unwrap();
	store.purge_snapshot(document);

	let continued = research
		.fetch(
			&call(run, "continue", "research.page@1"),
			&pages,
			&json!({"document_id": document, "cursor": cursor}),
		)
		.await
		.unwrap();
	assert_eq!(continued["error"]["code"], "snapshot_purged", "{continued}");
	let found = research
		.find(
			&call(run, "find", "research.page@1"),
			&json!({"document_id": document, "query": "line"}),
		)
		.await
		.unwrap();
	assert_eq!(found["error"]["code"], "snapshot_purged", "{found}");
	// A document this Run never read is still reported as unknown.
	let unknown = research
		.fetch(
			&call(run, "unknown", "research.page@1"),
			&pages,
			&json!({"document_id": Uuid::new_v4()}),
		)
		.await
		.unwrap();
	assert_eq!(unknown["error"]["code"], "unknown_document");
	// The delivered fragment still supports its citation.
	assert_eq!(store.fragments(run, &[reference]).await.unwrap().len(), 1);
	// A purged snapshot no longer counts toward the snapshot limits.
	let usage = store.usage(run).await.unwrap();
	assert_eq!((usage.snapshots, usage.snapshot_bytes), (0, 0));

	let again = research
		.fetch(
			&call(run, "again", "research.page@1"),
			&pages,
			&json!({"source_id": read["data"]["source_id"]}),
		)
		.await
		.unwrap();
	assert_eq!(again["status"], "ok", "{again}");
	assert_ne!(again["data"]["document_id"], read["data"]["document_id"]);
	assert_eq!(pages.calls.load(Ordering::SeqCst), 2);
}

/// Reads Runs as a fixed set; every other Run, existing or not, is unreadable.
struct Readers(Vec<Uuid>);

#[async_trait]
impl crate::ports::research::RunReader for Readers {
	async fn readable(&self, run: Uuid) -> Result<bool> {
		Ok(self.0.contains(&run))
	}
}

/// A Run that fetched one page and found one of its lines, as evidence to cite.
async fn cited(store: &Memory, run: Uuid) -> EvidenceRef {
	let (gate, pages) = (Gate::permitting(), Pages::new());
	let research = Research {
		store,
		gate: &gate,
		limits: Limits::default(),
	};
	let fetched = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source","max_bytes":5}),
		)
		.await
		.unwrap();
	let found = research
		.find(
			&call(run, "k2", "research.page@1"),
			&json!({"document_id":fetched["data"]["document_id"],"query":"strasse"}),
		)
		.await
		.unwrap();
	found["data"]["matches"][0]["evidence_ref"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap()
}

#[tokio::test]
async fn a_reader_of_the_run_opens_its_cited_fragment_with_snapshot_context() {
	// Arrange
	let store = Memory::default();
	let run = Uuid::new_v4();
	let reference = cited(&store, run).await;

	// Act
	let evidence = reading::read(&Readers(vec![run]), &store, run, &reference.to_string())
		.await
		.unwrap();

	// Assert
	assert_eq!(evidence.fragment.reference, reference);
	assert_eq!(evidence.fragment.run_id, run);
	assert_eq!(
		evidence.fragment.text,
		"Die Straße <script>inert text</script>"
	);
	assert_eq!(
		(evidence.fragment.line_start, evidence.fragment.line_end),
		(2, 2)
	);
	assert_eq!(evidence.url, "https://example.com/source");
	assert_eq!(evidence.title.as_deref(), Some("Source"));
	let context = evidence.context.unwrap();
	assert_eq!(context.before, [Pages::new().lines[0].clone()]);
	assert_eq!(context.after, [Pages::new().lines[2].clone()]);
}

/// Reads the Run until a revocation lands after its first `readable` check.
struct RevokedAfterFirstCheck {
	run: Uuid,
	checks: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl crate::ports::research::RunReader for RevokedAfterFirstCheck {
	async fn readable(&self, run: Uuid) -> Result<bool> {
		let check = self
			.checks
			.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		Ok(run == self.run && check == 0)
	}
}

#[tokio::test]
async fn a_revocation_during_the_evidence_read_withholds_the_evidence() {
	// Arrange: the reader loses access after the first readability decision.
	let store = Memory::default();
	let run = Uuid::new_v4();
	let reference = cited(&store, run).await;
	let reader = RevokedAfterFirstCheck {
		run,
		checks: Default::default(),
	};

	// Act
	let error = reading::read(&reader, &store, run, &reference.to_string())
		.await
		.unwrap_err();

	// Assert: the same absence as any other unreadable evidence.
	assert_eq!(error.to_string(), reading::unavailable().to_string());
	assert_eq!(
		reader.checks.load(std::sync::atomic::Ordering::SeqCst),
		2,
		"readability is decided again after the evidence read"
	);
}

#[tokio::test]
async fn evidence_revoked_during_the_read_is_withheld() {
	// Arrange: the revocation commits after the fragment and snapshot reads.
	let store = Memory::default();
	let run = Uuid::new_v4();
	let reference = cited(&store, run).await;
	store
		.revoke_after_snapshot_read
		.store(true, Ordering::SeqCst);

	// Act
	let error = reading::read(&Readers(vec![run]), &store, run, &reference.to_string())
		.await
		.unwrap_err();

	// Assert
	assert_eq!(error.to_string(), reading::unavailable().to_string());
}

#[tokio::test]
async fn a_page_dated_after_its_retrieval_is_never_retained() {
	let store = Memory::default();
	let (gate, pages) = (Gate::permitting(), Pages::new());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let fetched = research
		.fetch(
			&call(Uuid::new_v4(), "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://future.example.com/page","max_bytes":5}),
		)
		.await
		.unwrap();
	assert_eq!(
		fetched["error"]["code"], "provider_response_invalid",
		"{fetched}"
	);
	assert!(store.snapshots.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_revocation_committed_before_the_reservation_dispatches_nothing() {
	// Arrange: the Run passes its first revocation check, then is revoked.
	let store = Memory::default();
	let (gate, pages) = (Gate::permitting(), Pages::new());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	store.revoke_at_reserve.store(true, Ordering::SeqCst);

	// Act
	let fetched = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source","max_bytes":5}),
		)
		.await
		.unwrap();

	// Assert: the locked reservation refused; no attempt, no page read.
	assert_eq!(fetched["error"]["code"], "evidence_revoked", "{fetched}");
	assert!(store.attempts.lock().unwrap().is_empty());
	assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_snapshot_purged_before_its_fragment_is_never_delivered() {
	// Arrange: a retained snapshot, purged after the read that loads it.
	let store = Memory::default();
	let (gate, pages) = (Gate::permitting(), Pages::new());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	let fetched = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&pages,
			&json!({"url":"https://example.com/source","max_bytes":5}),
		)
		.await
		.unwrap();
	let fragments = store.fragments.lock().unwrap().len();
	store.purge_at_fragment.store(true, Ordering::SeqCst);

	// Act
	let found = research
		.find(
			&call(run, "k2", "research.page@1"),
			&json!({"document_id":fetched["data"]["document_id"],"query":"strasse"}),
		)
		.await
		.unwrap();

	// Assert: no text and no new fragment.
	assert_eq!(found["error"]["code"], "snapshot_purged", "{found}");
	assert_eq!(store.fragments.lock().unwrap().len(), fragments);
}

#[tokio::test]
async fn every_reason_a_reader_cannot_see_evidence_is_the_same_absence() {
	// Arrange: one Run with evidence, another readable Run, and an unknown Run.
	let store = Memory::default();
	let (run, other) = (Uuid::new_v4(), Uuid::new_v4());
	let reference = cited(&store, run).await.to_string();
	let unknown = EvidenceRef(Uuid::new_v4()).to_string();
	let readable = Readers(vec![run, other]);
	let cases = [
		(
			"the reader cannot read the Run",
			Readers(vec![other]),
			run,
			reference.clone(),
		),
		(
			"the Run does not exist",
			Readers(vec![]),
			Uuid::new_v4(),
			reference.clone(),
		),
		(
			"the reference is another Run's",
			readable,
			other,
			reference.clone(),
		),
		(
			"the reference does not exist",
			Readers(vec![run]),
			run,
			unknown,
		),
		(
			"the reference is malformed",
			Readers(vec![run]),
			run,
			"ev_nope".into(),
		),
	];

	for (case, reader, run, reference) in cases {
		// Act
		let error = reading::read(&reader, &store, run, &reference)
			.await
			.unwrap_err();

		// Assert: identical, and indistinguishable from any other absence.
		assert!(
			matches!(&error, Error::NotFound(message) if message == "evidence unavailable"),
			"{case}: {error:?}"
		);
		assert_eq!(
			error.to_string(),
			reading::unavailable().to_string(),
			"{case}"
		);
	}
}

fn scanned(
	lines: &[(&str, u32)],
	pages_without_text: Vec<u32>,
	original_bytes: usize,
) -> ScannedPdf {
	ScannedPdf {
		lines: lines
			.iter()
			.map(|(text, page)| Line {
				text: (*text).into(),
				page: Some(*page),
			})
			.collect(),
		pages_without_text,
		original_bytes,
	}
}

#[tokio::test]
async fn pages_without_text_are_reported_with_whether_they_can_be_viewed() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	let read = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&scanned(
				&[("Page one has text.", 1), ("So does page three.", 3)],
				vec![2],
				1024,
			),
			&json!({"url":"https://example.com/report.pdf"}),
		)
		.await
		.unwrap();
	assert_eq!(read["status"], "ok", "{read}");
	assert_eq!(read["data"]["pages"], json!([1, 3]));
	assert_eq!(read["data"]["page_count"], 3);
	assert_eq!(read["data"]["pages_without_text"]["pages"], json!([2]));
	assert_eq!(
		read["data"]["pages_without_text"]["page_images"],
		"available"
	);
	assert_eq!(store.originals.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_document_whose_pages_all_lack_text_delivers_nothing_but_its_pages() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits::default(),
	};
	let run = Uuid::new_v4();
	let read = research
		.fetch(
			&call(run, "k1", "research.page@1"),
			&scanned(&[], vec![1, 2, 3], 1024),
			&json!({"url":"https://example.com/scan.pdf"}),
		)
		.await
		.unwrap();
	assert_eq!(read["status"], "empty", "{read}");
	assert!(read["data"]["evidence_ref"].is_null());
	assert_eq!(
		read["data"]["pages_without_text"]["pages"],
		json!([1, 2, 3])
	);
	assert!(store.fragments.lock().unwrap().is_empty());
	// Reading the snapshot again by its identifier delivers nothing either.
	let again = research
		.fetch(
			&call(run, "k2", "research.page@1"),
			&scanned(&[], vec![1, 2, 3], 1024),
			&json!({"document_id": read["data"]["document_id"]}),
		)
		.await
		.unwrap();
	assert_eq!(again["status"], "empty", "{again}");
}

#[tokio::test]
async fn originals_beyond_the_run_limit_are_not_retained() {
	let (store, gate) = (Memory::default(), Gate::permitting());
	let research = Research {
		store: &store,
		gate: &gate,
		limits: Limits {
			original_bytes: 1500,
			..Limits::default()
		},
	};
	let run = Uuid::new_v4();
	let mut states = Vec::new();
	for (key, address) in [
		("k1", "https://example.com/a.pdf"),
		("k2", "https://example.com/b.pdf"),
	] {
		let read = research
			.fetch(
				&call(run, key, "research.page@1"),
				&scanned(&[("Text on page one.", 1)], vec![2], 1024),
				&json!({"url": address}),
			)
			.await
			.unwrap();
		states.push(read["data"]["pages_without_text"]["page_images"].clone());
	}
	assert_eq!(states, [json!("available"), json!("original_not_retained")]);
	assert_eq!(store.originals.lock().unwrap().len(), 1);
}

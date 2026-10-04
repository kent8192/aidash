use super::*;
use crate::{
	ports::{
		EmbeddingProvider, VectorIndex,
		generation::dispatch::{
			DispatchPreparation, DispatchVisibility, GenerationDispatchRepository,
			GenerationDispatchSettlement,
		},
		semantic::{
			remote_journal::{JournalRepository, JournalScope},
			retrieval::SemanticRetrievalSession,
		},
	},
	semantic::retrieval,
};
use aidash_domain::{
	Message, Task,
	federation::execution::{Description, Inspection, home::HomeBinding},
	generation::{
		dispatch::{FinalizeInput, Record as DispatchRecord},
		remote::{Finalization, Reserved, Usage},
	},
	semantic::{
		EmbeddingConfig, Source, VectorConfig,
		indexing::IndexingSpec,
		mutations::{Entry, Index},
		remote::{
			Boundary, InputRead, Provider, SourceRead,
			journal::{Attempt, Record as JournalRecord},
		},
		results::SearchResult,
	},
};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use rstest::rstest;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
type Trace = Arc<Mutex<Vec<&'static str>>>;
struct NoEffects;
#[async_trait]
impl GenerationDispatchRepository for NoEffects {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	async fn begin_preparation(&self) -> Result<Box<dyn DispatchPreparation>> {
		panic!("unexpected begin_preparation effect in disclosure scenario")
	}
	async fn record(&self, _: Uuid) -> Result<Option<DispatchRecord>> {
		panic!("unexpected record effect in disclosure scenario")
	}
	async fn admit(&self, _: &Input, _: &[Reserved]) -> Result<u64> {
		panic!("unexpected admit effect in disclosure scenario")
	}
	async fn finalize(&self, _: Uuid, _: &str, _: &str, _: &Value) -> Result<u64> {
		panic!("unexpected finalize effect in disclosure scenario")
	}
	async fn begin_visibility(&self) -> Result<Box<dyn DispatchVisibility>> {
		panic!("unexpected begin_visibility effect in disclosure scenario")
	}
	async fn mark_peer_finalized(&self, _: Uuid) -> Result<()> {
		panic!("unexpected mark_peer_finalized effect in disclosure scenario")
	}
}
#[async_trait]
impl GenerationDispatchSettlement for NoEffects {
	async fn local(&self, _: &Usage, _: &Finalization) -> Result<()> {
		panic!("unexpected local effect in disclosure scenario")
	}
	async fn peer(&self, _: &str, _: &FinalizeInput) -> Result<bool> {
		panic!("unexpected peer effect in disclosure scenario")
	}
}
#[async_trait]
impl EmbeddingProvider for NoEffects {
	async fn embed(
		&self,
		_: &aidash_domain::semantic::EmbeddingConfig,
		_: &str,
	) -> Result<aidash_domain::semantic::Embedding> {
		panic!("unexpected embed effect in disclosure scenario")
	}
}
#[async_trait]
impl VectorIndex for NoEffects {
	async fn ensure_collection(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
		_: usize,
	) -> Result<()> {
		panic!("unexpected ensure_collection effect in disclosure scenario")
	}
	async fn upsert(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
		_: uuid::Uuid,
		_: &[f32],
		_: serde_json::Value,
	) -> Result<()> {
		panic!("unexpected upsert effect in disclosure scenario")
	}
	async fn delete_point(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
		_: uuid::Uuid,
	) -> Result<()> {
		panic!("unexpected delete_point effect in disclosure scenario")
	}
	async fn delete_collection(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
	) -> Result<()> {
		panic!("unexpected delete_collection effect in disclosure scenario")
	}
	async fn query(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
		_: &[f32],
		_: aidash_domain::semantic::VectorFilter<'_>,
		_: usize,
	) -> Result<Vec<aidash_domain::semantic::Point>> {
		panic!("unexpected query effect in disclosure scenario")
	}
	async fn present(
		&self,
		_: &aidash_domain::semantic::VectorConfig,
		_: &str,
		_: &[uuid::Uuid],
	) -> Result<bool> {
		panic!("unexpected present effect in disclosure scenario")
	}
}
fn task() -> Task {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"title":"title","description":"intent","status":"OPEN","requirements":{},"owner":null,"created_by":"requester","dependencies":[],"parent_id":null,"revision":7,"created_at":"2026-10-04T00:00:00Z"})).unwrap()
}
fn message() -> Message {
	Message {
		id: Uuid::from_u128(5),
		workspace_id: task().workspace_id,
		sender: "requester".into(),
		content: "input text".into(),
		idempotency_key: None,
		created_at: Utc.timestamp_opt(0, 0).unwrap(),
	}
}
fn operation() -> Operation {
	let inputs = vec![InputRead {
		id: message().id,
		sequence: 1,
		digest: content_digest(&message().content),
	}];
	Operation {
		id: Uuid::from_u128(10),
		home_node: "aidash://home".into(),
		grant_id: Uuid::from_u128(3),
		admission_id: Uuid::from_u128(4),
		boundary: Boundary {
			step: 0,
			input_sequence: 1,
			task_revision: task().revision,
			inputs_digest: digest(&json!(inputs)),
		},
		inputs,
		query: format!(
			"{}\n{}\n{}",
			task().title,
			task().description,
			message().content
		),
		max_tokens: 8192,
		metadata: json!({}),
	}
}
fn binding() -> Binding {
	Binding::RequiredHome {
		home_lineage: vec![],
		execution_lineage: vec![],
		version: 1,
		index_revision: 9,
		index_digest: "index".into(),
		embedding: Box::new(Provider {
			node_id: "aidash://home".into(),
			entry: aidash_domain::registry::EntityRef {
				id: "embedding".into(),
				version: "1".into(),
			},
			digest: "provider".into(),
			configuration_digest: "configuration".into(),
		}),
		compactor: None,
	}
}
fn description() -> Description {
	let inspection:Inspection=serde_json::from_value(json!({"node_id":"aidash://receiver","authority_digest":"authority","agent":{"id":"agent","version":"1","kind":"agent","name":{},"description":{},"config":{"model":{"id":"model","version":"1"}}},"definitions":[]})).unwrap();
	Description {
		grant_id: operation().grant_id,
		source_node: "aidash://home".into(),
		target_node: "aidash://receiver".into(),
		source_tenant: "tenant".into(),
		source_subject: "requester".into(),
		task: task(),
		inspection,
		expires_at: Utc.timestamp_opt(3600, 0).unwrap(),
		semantic: binding(),
	}
}
fn spec() -> IndexingSpec {
	IndexingSpec {
		embedding: EmbeddingConfig {
			provider: "openai".into(),
			endpoint: "https://embedding.invalid".into(),
			credential_env: None,
			model: "embedding".into(),
			model_version: "1".into(),
			dimensions: 2,
		},
		vector: VectorConfig {
			provider: "qdrant".into(),
			endpoint: "https://vector.invalid".into(),
			credential_env: None,
		},
		enabled: true,
		auto_context: true,
		max_sources: 16,
		max_results: 4,
		max_result_tokens: 8192,
		max_input_bytes: 32768,
	}
}
fn index() -> Index {
	Index {
		workspace_id: task().workspace_id,
		tenant: "tenant".into(),
		revision: 9,
		spec: json!(spec()),
		collection: "collection".into(),
		updated_at: Utc.timestamp_opt(0, 0).unwrap(),
	}
}
fn cached_receipt(candidate_digest: &str) -> Receipt {
	Receipt {
		operation_id: operation().id,
		operation_digest: operation().digest().unwrap(),
		home_node: operation().home_node,
		tenant: "tenant".into(),
		workspace_id: task().workspace_id,
		grant_id: operation().grant_id,
		admission_id: operation().admission_id,
		executor: "aidash://receiver/agent@1".into(),
		binding: binding(),
		retrieved_at: Utc.timestamp_opt(0, 0).unwrap(),
		query_truncated: false,
		candidate_digest: candidate_digest.into(),
		sources: vec![],
		result: SearchResult {
			workspace_id: task().workspace_id,
			index_revision: 9,
			model: "embedding".into(),
			model_version: "1".into(),
			matches: vec![],
			estimated_tokens: 1,
			truncated: false,
		},
		estimated_tokens: 2,
	}
}
struct EmptyRetrieval;
#[async_trait]
impl SemanticRetrievalSession for EmptyRetrieval {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		assert_eq!(workspace, task().workspace_id);
		assert_eq!(action, "semantic.search");
		Ok(())
	}
	async fn index(&mut self, _: Uuid) -> Result<Index> {
		Ok(index())
	}
	async fn configured(&mut self, _: Uuid) -> Result<Option<Index>> {
		panic!("unexpected configured effect in disclosure scenario")
	}
	async fn candidates(&mut self, _: Uuid) -> Result<Vec<Entry>> {
		Ok(vec![])
	}
	async fn permits(&mut self, _: &Entry, _: &str) -> Result<bool> {
		panic!("unexpected permits effect in disclosure scenario")
	}
	async fn source(&mut self, _: Uuid, _: &Source) -> Result<Option<String>> {
		panic!("unexpected source effect in disclosure scenario")
	}
	async fn digest(&mut self, _: Uuid) -> Result<String> {
		panic!("unexpected digest effect in disclosure scenario")
	}
	async fn embed(
		&mut self,
		_: Uuid,
		_: &EmbeddingConfig,
		_: &str,
		_: Option<Uuid>,
	) -> Result<Vec<f32>> {
		panic!("unexpected embed effect in disclosure scenario")
	}
}
struct Journal {
	trace: Trace,
	record: Arc<Mutex<JournalRecord>>,
}
impl Journal {
	fn new(trace: Trace) -> Self {
		let op = operation();
		let operation_digest = op.digest().unwrap();
		Self {
			trace,
			record: Arc::new(Mutex::new(JournalRecord {
				id: op.id,
				home_node: op.home_node,
				grant_id: op.grant_id,
				admission_id: op.admission_id,
				digest: operation_digest,
				binding: json!({"operation":operation(),"semantic":binding()}),
				state: "WAITING".into(),
				cycle: 0,
				failures: 0,
				attempt_id: None,
				fence: 0,
				lease_until: None,
				next_attempt: None,
				error: None,
				receipt: None,
			})),
		}
	}
}
struct JournalLease {
	trace: Trace,
	record: Arc<Mutex<JournalRecord>>,
}
#[async_trait]
impl JournalRepository for Journal {
	async fn begin(&self) -> Result<Box<dyn JournalScope + '_>> {
		self.trace.lock().unwrap().push("journal");
		Ok(Box::new(JournalLease {
			trace: self.trace.clone(),
			record: self.record.clone(),
		}))
	}
	async fn bound(&self, _: Uuid) -> Result<JournalRecord> {
		panic!("search does not separately reread journal binding")
	}
	async fn expire_cached(&self, id: Uuid) -> Result<()> {
		assert_eq!(id, operation().id);
		self.trace.lock().unwrap().push("expire_cache");
		let mut record = self.record.lock().unwrap();
		record.state = "WAITING".into();
		record.receipt = None;
		Ok(())
	}
}
#[async_trait]
impl JournalScope for JournalLease {
	async fn insert_binding(&mut self, op: &Operation, digest: &str, value: &Value) -> Result<()> {
		assert_eq!(op.id, operation().id);
		assert_eq!(digest, self.record.lock().unwrap().digest);
		assert_eq!(*value, self.record.lock().unwrap().binding);
		Ok(())
	}
	async fn shared(&mut self, _: Uuid) -> Result<JournalRecord> {
		Ok(self.record.lock().unwrap().clone())
	}
	async fn locked(&mut self, _: Uuid) -> Result<JournalRecord> {
		Ok(self.record.lock().unwrap().clone())
	}
	async fn clock(&mut self) -> Result<DateTime<Utc>> {
		Ok(Utc.timestamp_opt(0, 0).unwrap())
	}
	async fn expire_attempt(&mut self, _: Uuid) -> Result<()> {
		panic!("unexpected expire_attempt effect in disclosure scenario")
	}
	async fn wait_expired(
		&mut self,
		_: Uuid,
		_: i32,
		_: Option<i64>,
		_: &str,
		_: &str,
	) -> Result<()> {
		panic!("unexpected wait_expired effect in disclosure scenario")
	}
	async fn activate(&mut self, _: Uuid, attempt: &Attempt, _: &JournalRecord) -> Result<()> {
		self.trace.lock().unwrap().push("claim");
		let mut r = self.record.lock().unwrap();
		r.state = "ACTIVE".into();
		r.attempt_id = Some(attempt.id);
		r.fence = attempt.fence;
		Ok(())
	}
	async fn current(&mut self, attempt: &Attempt) -> Result<JournalRecord> {
		let r = self.record.lock().unwrap().clone();
		assert_eq!(r.attempt_id, Some(attempt.id));
		assert_eq!(r.fence, attempt.fence);
		Ok(r)
	}
	async fn dispatched(&mut self, _: &Attempt, _: &Value) -> Result<u64> {
		panic!("unexpected dispatched effect in disclosure scenario")
	}
	async fn record_source(&mut self, _: &Receipt, _: &SourceRead) -> Result<()> {
		panic!("unexpected record_source effect in disclosure scenario")
	}
	async fn complete_operation(&mut self, receipt: &Receipt) -> Result<()> {
		self.trace.lock().unwrap().push("complete");
		let mut r = self.record.lock().unwrap();
		r.state = "READY".into();
		r.receipt = Some(json!(receipt));
		Ok(())
	}
	async fn complete_attempt(&mut self, _: &Attempt) -> Result<()> {
		Ok(())
	}
	async fn fail_operation(
		&mut self,
		_: &JournalRecord,
		_: i32,
		_: Option<i64>,
		_: &str,
		_: &str,
	) -> Result<()> {
		panic!("unexpected fail_operation effect in disclosure scenario")
	}
	async fn fail_attempt(&mut self, _: &Attempt, _: &str) -> Result<()> {
		panic!("unexpected fail_attempt effect in disclosure scenario")
	}
	async fn resume_records(&mut self, _: Uuid, _: Uuid) -> Result<Vec<JournalRecord>> {
		panic!("unexpected resume_records effect in disclosure scenario")
	}
	async fn resume_record(&mut self, _: &JournalRecord) -> Result<()> {
		panic!("unexpected resume_record effect in disclosure scenario")
	}
	async fn event(&mut self, _: Uuid, _: &str, _: Value) -> Result<()> {
		panic!("unexpected event effect in disclosure scenario")
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(())
	}
}
#[derive(Default)]
struct Authority {
	active: usize,
	opened: usize,
}
struct Repository {
	trace: Trace,
	authority: Arc<Mutex<Authority>>,
	journal: Journal,
	effects: NoEffects,
	denied: Option<&'static str>,
	revoked_at_delivery: bool,
	blocked: bool,
}
impl Repository {
	fn new() -> Self {
		let trace = Trace::default();
		Self {
			trace: trace.clone(),
			authority: Default::default(),
			journal: Journal::new(trace),
			effects: NoEffects,
			denied: None,
			revoked_at_delivery: false,
			blocked: false,
		}
	}
	fn cached(&self, candidate: &str) {
		let mut r = self.journal.record.lock().unwrap();
		r.state = "READY".into();
		r.receipt = Some(json!(cached_receipt(candidate)));
	}
}
struct Scope {
	trace: Trace,
	authority: Arc<Mutex<Authority>>,
	denied: Option<&'static str>,
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.authority.lock().unwrap().active -= 1;
		self.trace.lock().unwrap().push("drop_lease");
	}
}
#[async_trait]
impl SemanticSearchRepository for Repository {
	type Scope = Scope;
	fn home_node_id(&self) -> &str {
		"aidash://home"
	}
	fn journal(&self) -> &dyn JournalRepository {
		&self.journal
	}
	fn dispatch(&self) -> &dyn GenerationDispatchRepository {
		&self.effects
	}
	fn settlement(&self) -> &dyn GenerationDispatchSettlement {
		&self.effects
	}
	fn embedding(&self) -> &dyn EmbeddingProvider {
		&self.effects
	}
	fn vector(&self) -> &dyn VectorIndex {
		&self.effects
	}
	async fn description(&self, node: &str, grant: Uuid) -> Result<(Scope, Description)> {
		assert_eq!(node, "aidash://receiver");
		assert_eq!(grant, operation().grant_id);
		let mut authority = self.authority.lock().unwrap();
		assert_eq!(authority.active, 0);
		self.trace.lock().unwrap().push("description");
		if authority.opened > 0 && self.revoked_at_delivery {
			return Err(Error::Forbidden);
		}
		authority.opened += 1;
		authority.active += 1;
		let mut description = description();
		if self.denied == Some("disabled") {
			description.semantic = Binding::Disabled {};
		}
		Ok((
			Scope {
				trace: self.trace.clone(),
				authority: self.authority.clone(),
				denied: self.denied,
			},
			description,
		))
	}
	async fn verify_operation(&self, node: &str, op: &Operation) -> Result<bool> {
		assert_eq!(node, "aidash://receiver");
		assert_eq!(op.id, operation().id);
		assert_eq!(self.authority.lock().unwrap().active, 1);
		self.trace.lock().unwrap().push("receiver");
		if self.blocked {
			std::future::pending::<()>().await;
		}
		Ok(self.denied != Some("receiver"))
	}
	async fn receiver_reservations(&self, _: &str, _: &Input) -> Result<Vec<Reserved>> {
		panic!("empty candidates must not reserve provider usage")
	}
}
#[async_trait]
impl SemanticSearchScope for Scope {
	async fn admission_binding(&mut self, grant: Uuid) -> Result<Option<HomeBinding>> {
		assert_eq!(grant, operation().grant_id);
		self.trace.lock().unwrap().push("binding");
		if self.denied == Some("binding") {
			return Ok(None);
		}
		Ok(Some(HomeBinding {
			grant_id: grant,
			admission_id: if self.denied == Some("admission") {
				Uuid::from_u128(99)
			} else {
				operation().admission_id
			},
			task_id: task().id,
			task_revision: task().revision,
			initial_task: json!(task()),
		}))
	}
	fn disclosure_mode(&mut self, grant: Uuid) -> Result<()> {
		assert_eq!(grant, operation().grant_id);
		self.trace.lock().unwrap().push("worker");
		Ok(())
	}
	async fn disclosure_task(&mut self, _: Uuid) -> Result<Task> {
		let mut task = task();
		if self.denied == Some("revision") {
			task.revision += 1;
		}
		Ok(task)
	}
	async fn input_message(&mut self, workspace: Uuid, id: Uuid) -> Result<Message> {
		assert_eq!(workspace, task().workspace_id);
		assert_eq!(id, message().id);
		let mut value = message();
		if self.denied == Some("input") {
			value.content.push_str(" changed");
		}
		Ok(value)
	}
	async fn index_spec(&mut self, _: Uuid) -> Result<IndexingSpec> {
		Ok(spec())
	}
	async fn prepare_search(
		&mut self,
		workspace: Uuid,
		input: &Search,
		controls: &AgentConfig,
	) -> Result<retrieval::PreparedSearch> {
		assert_eq!(
			input.agent.as_deref(),
			Some(qualified_agent("aidash://receiver", "agent", "1").as_str())
		);
		assert_eq!(controls.model.id, "model");
		self.trace.lock().unwrap().push("candidates");
		retrieval::prepare(&mut EmptyRetrieval, workspace, input, Some(controls)).await
	}
	async fn reserve_home(&mut self, _: &Usage) -> Result<Vec<Reserved>> {
		panic!("empty candidates must not debit Home allowance")
	}
	async fn finish_search(
		&mut self,
		_: retrieval::PreparedSearch,
		_: &[f32],
	) -> Result<SearchResult> {
		panic!("empty candidates must not query vectors")
	}
	async fn finish(self, result: Result<Receipt>) -> Result<Receipt> {
		self.trace.lock().unwrap().push(if result.is_ok() {
			"finish_ok"
		} else {
			"finish_denied"
		});
		result
	}
}
#[rstest]
#[case("disabled")]
#[case("binding")]
#[case("admission")]
#[case("revision")]
#[case("input")]
#[case("query")]
#[case("input_digest")]
#[case("receiver")]
#[tokio::test]
async fn current_authority_and_complete_input_boundary_precede_journal_or_provider_effects(
	#[case] denied: &'static str,
) {
	let mut repo = Repository::new();
	repo.denied = Some(denied);
	let mut op = operation();
	if denied == "query" {
		op.query = "forged query".into();
	} else if denied == "input_digest" {
		op.boundary.inputs_digest = "forged inputs".into();
	}
	let result = search(&repo, "aidash://receiver", op).await;
	if matches!(denied, "disabled" | "input") {
		assert!(matches!(result, Err(Error::RemoteSemantic(_))));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(repo.authority.lock().unwrap().active, 0);
	let trace = repo.trace.lock().unwrap();
	assert!(!trace.contains(&"journal"));
	assert!(!trace.contains(&"claim"));
	assert!(trace.contains(&"finish_denied"));
	if denied != "receiver" {
		assert!(!trace.contains(&"receiver"));
	}
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn invalid_operation_or_wrong_home_is_rejected_before_any_source_lease(
	#[case] malformed: bool,
) {
	let repo = Repository::new();
	let mut op = operation();
	if malformed {
		op.max_tokens = 0;
	} else {
		op.home_node = "aidash://other".into();
	}
	assert!(search(&repo, "aidash://receiver", op).await.is_err());
	assert!(repo.trace.lock().unwrap().is_empty());
	assert_eq!(repo.authority.lock().unwrap().opened, 0);
}
#[rstest]
#[tokio::test]
async fn empty_candidates_complete_one_attempt_without_provider_effects_and_reacquire_authority() {
	let repo = Repository::new();
	let receipt = search(&repo, "aidash://receiver", operation())
		.await
		.unwrap();
	assert_eq!(receipt.operation_id, operation().id);
	assert_eq!(receipt.operation_digest, operation().digest().unwrap());
	assert!(receipt.result.matches.is_empty());
	assert!(receipt.sources.is_empty());
	assert_eq!(repo.authority.lock().unwrap().active, 0);
	assert_eq!(repo.authority.lock().unwrap().opened, 2);
	assert_eq!(
		*repo.trace.lock().unwrap(),
		vec![
			"description",
			"binding",
			"worker",
			"receiver",
			"journal",
			"candidates",
			"journal",
			"claim",
			"journal",
			"complete",
			"finish_ok",
			"drop_lease",
			"description",
			"finish_ok",
			"drop_lease"
		]
	);
}
#[rstest]
#[tokio::test]
async fn cached_receipt_replays_only_when_current_candidate_identity_still_matches() {
	let repo = Repository::new();
	repo.cached(&digest(&json!([])));
	let receipt = search(&repo, "aidash://receiver", operation())
		.await
		.unwrap();
	assert_eq!(receipt.retrieved_at, Utc.timestamp_opt(0, 0).unwrap());
	assert_eq!(repo.authority.lock().unwrap().opened, 2);
	let trace = repo.trace.lock().unwrap();
	assert!(!trace.contains(&"claim"));
	assert!(!trace.contains(&"complete"));
	assert!(!trace.contains(&"expire_cache"));
}
#[rstest]
#[tokio::test]
async fn changed_candidate_set_expires_cached_receipt_before_claiming_a_new_attempt() {
	let repo = Repository::new();
	repo.cached("different candidates");
	let receipt = search(&repo, "aidash://receiver", operation())
		.await
		.unwrap();
	assert_eq!(receipt.candidate_digest, digest(&json!([])));
	let trace = repo.trace.lock().unwrap();
	let expiry = trace.iter().position(|v| *v == "expire_cache").unwrap();
	let claim = trace.iter().position(|v| *v == "claim").unwrap();
	assert!(expiry < claim);
	assert!(trace.contains(&"complete"));
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn queued_revocation_is_observed_after_release_before_cached_or_new_receipt_delivery(
	#[case] cached: bool,
) {
	let mut repo = Repository::new();
	repo.revoked_at_delivery = true;
	if cached {
		repo.cached(&digest(&json!([])));
	}
	assert!(matches!(
		search(&repo, "aidash://receiver", operation()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repo.authority.lock().unwrap().active, 0);
	let trace = repo.trace.lock().unwrap();
	assert_eq!(
		&trace[trace.len() - 3..],
		["finish_ok", "drop_lease", "description"]
	);
}
#[rstest]
#[tokio::test]
async fn cancelling_receiver_verification_drops_the_original_credential_scope() {
	let mut repo = Repository::new();
	repo.blocked = true;
	let mut pending = Box::pin(search(&repo, "aidash://receiver", operation()));
	assert!(futures_util::poll!(&mut pending).is_pending());
	assert_eq!(repo.authority.lock().unwrap().active, 1);
	drop(pending);
	assert_eq!(repo.authority.lock().unwrap().active, 0);
	let trace = repo.trace.lock().unwrap();
	assert_eq!(trace.last(), Some(&"drop_lease"));
	assert!(!trace.contains(&"journal"));
}

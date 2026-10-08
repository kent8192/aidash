use super::*;
use crate::ports::VectorIndex;
use aidash_domain::semantic::{
	EmbeddingConfig, Point, Source, VectorConfig, VectorFilter,
	indexing::{IndexingEntry, IndexingPlan},
};
use async_trait::async_trait;
use rstest::rstest;
use std::{
	collections::BTreeSet,
	sync::{Arc, Mutex},
};

#[derive(Clone)]
struct Fixture(Arc<Mutex<State>>);
struct State {
	trace: Vec<String>,
	faults: BTreeSet<String>,
	restore_denial: Option<&'static str>,
	exists: bool,
	locked: bool,
	workspace_denied: bool,
	write: bool,
	read: bool,
	authority: Value,
	current: Value,
	text: Option<String>,
	plan: IndexingPlan,
	entry: IndexingEntry,
	old: Option<(Option<String>, bool)>,
	present: bool,
	pending_embed: bool,
	payload: Option<Value>,
	next_point: Option<Uuid>,
	retry: Option<(i32, f64)>,
	points: Vec<Uuid>,
	collections: Vec<String>,
	cleanup_locked: bool,
	cleanup_failure: Vec<Option<String>>,
}
fn vector_config() -> VectorConfig {
	VectorConfig {
		provider: "postgres".into(),
		endpoint: "https://fixture.invalid".into(),
		credential_env: None,
	}
}
impl Default for Fixture {
	fn default() -> Self {
		let authority = json!({"credential":"current", "subjects":["user","agent"]});
		Self(Arc::new(Mutex::new(State {
			trace: vec![],
			faults: BTreeSet::new(),
			restore_denial: None,
			exists: true,
			locked: true,
			workspace_denied: false,
			write: true,
			read: true,
			current: authority.clone(),
			authority,
			text: Some("source text".into()),
			plan: IndexingPlan {
				collection: "physical_collection".into(),
				tenant: "tenant".into(),
				revision: 7,
				spec: json!({"embedding":{"provider":"openai","endpoint":"https://fixture.invalid","credential_env":null,"model":"approved","model_version":"1","dimensions":3},"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":true,"max_sources":32,"max_results":10,"max_result_tokens":1024,"max_input_bytes":4096}),
			},
			entry: IndexingEntry {
				id: Uuid::from_u128(1),
				workspace_id: Uuid::from_u128(2),
				source: json!({"kind":"memory","text":"source text"}),
				revision: 3,
				point_id: Uuid::from_u128(4),
				state: "PENDING".into(),
				attempts: 0,
			},
			old: None,
			present: false,
			pending_embed: false,
			payload: None,
			next_point: None,
			retry: None,
			points: vec![],
			collections: vec![],
			cleanup_locked: true,
			cleanup_failure: vec![],
		})))
	}
}
impl Fixture {
	fn record(&self, action: &str) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		state.trace.push(action.into());
		if state.faults.contains(action) {
			Err(Error::External(format!("fault:{action}")))
		} else {
			Ok(())
		}
	}
	fn trace(&self) -> Vec<String> {
		self.0.lock().unwrap().trace.clone()
	}
	fn fault(&self, action: &str) {
		self.0.lock().unwrap().faults.insert(action.into());
	}
	fn id(&self) -> Uuid {
		self.0.lock().unwrap().entry.id
	}
	fn has(&self, action: &str) -> bool {
		self.trace().iter().any(|actual| actual == action)
	}
}
struct Visibility(Fixture);
#[async_trait]
impl SemanticVisibility for Visibility {}
impl Drop for Visibility {
	fn drop(&mut self) {
		self.0
			.0
			.lock()
			.unwrap()
			.trace
			.push("visibility_drop".into());
	}
}
struct Session {
	fixture: Fixture,
	closed: bool,
}
impl Drop for Session {
	fn drop(&mut self) {
		if !self.closed {
			self.fixture
				.0
				.lock()
				.unwrap()
				.trace
				.push("rollback_drop".into());
		}
	}
}
struct Cleanup {
	fixture: Fixture,
	closed: bool,
}
impl Drop for Cleanup {
	fn drop(&mut self) {
		if !self.closed {
			self.fixture
				.0
				.lock()
				.unwrap()
				.trace
				.push("cleanup_rollback".into());
		}
	}
}

#[async_trait]
impl SemanticIndexingRepository for Fixture {
	async fn begin_visibility(&self) -> Result<Box<dyn SemanticVisibility>> {
		self.record("visibility")?;
		Ok(Box::new(Visibility(self.clone())))
	}
	async fn due(&self) -> Result<Vec<Uuid>> {
		self.record("due")?;
		Ok(vec![self.id()])
	}
	async fn initial(&self, _: Uuid) -> Result<Option<(Uuid, Value)>> {
		self.record("initial")?;
		let s = self.0.lock().unwrap();
		Ok(s.exists
			.then(|| (s.entry.workspace_id, s.authority.clone())))
	}
	async fn restore(&self, _: &Value) -> Result<Box<dyn SemanticIndexingSession>> {
		self.record("restore")?;
		match self.0.lock().unwrap().restore_denial {
			Some("forbidden") => return Err(Error::Forbidden),
			Some("unauthorized") => return Err(Error::Unauthorized),
			Some("missing") => return Err(Error::NotFound("identity".into())),
			_ => {}
		}
		Ok(Box::new(Session {
			fixture: self.clone(),
			closed: false,
		}))
	}
	async fn revoke(&self, _: Uuid, _: Uuid, authority: &Value) -> Result<()> {
		assert_eq!(*authority, self.0.lock().unwrap().authority);
		self.record("revoke_authority")
	}
	async fn cleanup_due(&self) -> Result<CleanupBatch> {
		self.record("cleanup_due")?;
		let s = self.0.lock().unwrap();
		Ok(CleanupBatch {
			points: s.points.clone(),
			collections: s.collections.clone(),
		})
	}
	async fn begin_cleanup(&self) -> Result<Box<dyn SemanticCleanupSession>> {
		self.record("cleanup_begin")?;
		Ok(Box::new(Cleanup {
			fixture: self.clone(),
			closed: false,
		}))
	}
}
#[async_trait]
impl SemanticIndexingSession for Session {
	fn durable(&mut self) {
		self.fixture.record("durable").unwrap();
	}
	async fn workspace_write(&mut self, _: Uuid) -> Result<()> {
		self.fixture.record("workspace")?;
		if self.fixture.0.lock().unwrap().workspace_denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn plan(&mut self, _: Uuid) -> Result<IndexingPlan> {
		self.fixture.record("plan")?;
		Ok(self.fixture.0.lock().unwrap().plan.clone())
	}
	async fn lock_entry(&mut self, _: Uuid) -> Result<Option<IndexingEntry>> {
		self.fixture.record("entry")?;
		let s = self.fixture.0.lock().unwrap();
		Ok(s.locked.then(|| s.entry.clone()))
	}
	async fn authority(&mut self, _: Uuid) -> Result<Value> {
		self.fixture.record("authority")?;
		Ok(self.fixture.0.lock().unwrap().current.clone())
	}
	async fn permits(&mut self, action: &str) -> Result<bool> {
		self.fixture.record(action)?;
		let s = self.fixture.0.lock().unwrap();
		Ok(if action == "semantic.write" {
			s.write
		} else {
			s.read
		})
	}
	async fn source(&mut self, _: Uuid, _: &Source) -> Result<Option<String>> {
		self.fixture.record("source")?;
		Ok(self.fixture.0.lock().unwrap().text.clone())
	}
	async fn set_revoked(&mut self, _: Uuid, reason: &str) -> Result<()> {
		self.fixture.record(&format!("revoked:{reason}"))
	}
	async fn retire_points(&mut self, _: Uuid) -> Result<()> {
		self.fixture.record("retire")
	}
	async fn history(&mut self, _: Uuid, _: Uuid, _: i64, state: &str, _: &str) -> Result<()> {
		self.fixture.record(&format!("history:{state}"))
	}
	async fn point(&mut self, _: Uuid) -> Result<Option<(Option<String>, bool)>> {
		self.fixture.record("point")?;
		Ok(self.fixture.0.lock().unwrap().old.clone())
	}
	async fn defer(&mut self, _: Uuid) -> Result<()> {
		self.fixture.record("defer")
	}
	async fn rotate(&mut self, _: Uuid, point: Uuid) -> Result<IndexingEntry> {
		self.fixture.record("rotate")?;
		let mut s = self.fixture.0.lock().unwrap();
		s.next_point = Some(point);
		s.entry.point_id = point;
		s.entry.revision += 1;
		s.entry.state = "PENDING".into();
		Ok(s.entry.clone())
	}
	async fn schedule_point(&mut self, _: &str) -> Result<()> {
		self.fixture.record("schedule")
	}
	async fn embed(
		&mut self,
		_: Uuid,
		config: &EmbeddingConfig,
		_: &str,
		_: Uuid,
	) -> Result<Vec<f32>> {
		assert_eq!(config.dimensions, 3);
		self.fixture.record("embed")?;
		let pending = self.fixture.0.lock().unwrap().pending_embed;
		if pending {
			std::future::pending::<()>().await;
		}
		Ok(vec![0.1, 0.2, 0.3])
	}
	async fn record_digest(&mut self, _: Uuid, digest: &str) -> Result<()> {
		assert_eq!(digest, indexing::content_digest("source text"));
		self.fixture.record("digest")
	}
	async fn ready(&mut self, _: Uuid) -> Result<()> {
		self.fixture.record("ready")
	}
	async fn failed(&mut self, _: Uuid, attempts: i32, delay: f64) -> Result<()> {
		self.fixture.record("failed")?;
		self.fixture.0.lock().unwrap().retry = Some((attempts, delay));
		Ok(())
	}
	async fn finish(mut self: Box<Self>, result: Result<bool>) -> Result<bool> {
		self.closed = true;
		self.fixture
			.record(if result.is_ok() { "commit" } else { "rollback" })?;
		result
	}
}
#[async_trait]
impl SemanticCleanupSession for Cleanup {
	async fn lock_point(&mut self, _: Uuid) -> Result<Option<(String, VectorConfig)>> {
		self.fixture.record("lock_point")?;
		Ok(self
			.fixture
			.0
			.lock()
			.unwrap()
			.cleanup_locked
			.then(|| ("physical_collection".into(), vector_config())))
	}
	async fn lock_collection(&mut self, _: &str) -> Result<Option<VectorConfig>> {
		self.fixture.record("lock_collection")?;
		Ok(self
			.fixture
			.0
			.lock()
			.unwrap()
			.cleanup_locked
			.then(vector_config))
	}
	async fn record_point(&mut self, _: Uuid, failure: Option<&str>) -> Result<()> {
		self.fixture.record("record_point")?;
		self.fixture
			.0
			.lock()
			.unwrap()
			.cleanup_failure
			.push(failure.map(str::to_owned));
		Ok(())
	}
	async fn record_collection(&mut self, _: &str, failure: Option<&str>) -> Result<()> {
		self.fixture.record("record_collection")?;
		self.fixture
			.0
			.lock()
			.unwrap()
			.cleanup_failure
			.push(failure.map(str::to_owned));
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.fixture.record("cleanup_commit")?;
		self.closed = true;
		Ok(())
	}
}
#[async_trait]
impl VectorIndex for Fixture {
	async fn ensure_collection(
		&self,
		_: &VectorConfig,
		collection: &str,
		dimensions: usize,
	) -> Result<()> {
		assert_eq!(collection, "physical_collection");
		assert_eq!(dimensions, 3);
		self.record("ensure")
	}
	async fn upsert(
		&self,
		_: &VectorConfig,
		collection: &str,
		point: Uuid,
		vector: &[f32],
		payload: Value,
	) -> Result<()> {
		assert_eq!(collection, "physical_collection");
		assert_eq!(point, self.0.lock().unwrap().entry.point_id);
		assert_eq!(vector, [0.1, 0.2, 0.3]);
		self.record("upsert")?;
		self.0.lock().unwrap().payload = Some(payload);
		Ok(())
	}
	async fn delete_point(&self, _: &VectorConfig, _: &str, _: Uuid) -> Result<()> {
		self.record("delete_point")
	}
	async fn delete_collection(&self, _: &VectorConfig, _: &str) -> Result<()> {
		self.record("delete_collection")
	}
	async fn query(
		&self,
		_: &VectorConfig,
		_: &str,
		_: &[f32],
		_: VectorFilter<'_>,
		_: usize,
	) -> Result<Vec<Point>> {
		panic!("indexing cannot query candidates")
	}
	async fn present(&self, _: &VectorConfig, _: &str, _: &[Uuid]) -> Result<bool> {
		self.record("present")?;
		Ok(self.0.lock().unwrap().present)
	}
}

#[rstest]
#[tokio::test]
async fn indexing_uses_both_permissions_and_commits_ready_after_external_acknowledgement() {
	let f = Fixture::default();
	let (id, workspace) = {
		let s = f.0.lock().unwrap();
		(s.entry.id, s.entry.workspace_id)
	};
	assert!(process(&f, &f, id).await.unwrap());
	assert_eq!(
		f.trace(),
		[
			"initial",
			"restore",
			"durable",
			"workspace",
			"plan",
			"entry",
			"authority",
			"semantic.write",
			"semantic.read",
			"source",
			"point",
			"ensure",
			"embed",
			"upsert",
			"digest",
			"ready",
			"history:READY",
			"commit"
		]
	);
	assert_eq!(
		f.0.lock().unwrap().payload,
		Some(
			json!({"entry_id":id,"revision":3,"index_revision":7,"workspace_id":workspace,"tenant":"tenant"})
		)
	);
}
#[rstest]
#[case::forbidden("forbidden")]
#[case::unauthorized("unauthorized")]
#[case::missing("missing")]
#[tokio::test]
async fn revoked_saved_authority_retires_only_the_exact_persisted_authority(
	#[case] denial: &'static str,
) {
	let f = Fixture::default();
	f.0.lock().unwrap().restore_denial = Some(denial);
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert_eq!(f.trace(), ["initial", "restore", "revoke_authority"]);
}
#[rstest]
#[case::initial("initial")]
#[case::restore("restore")]
#[case::workspace("workspace")]
#[case::plan("plan")]
#[case::entry("entry")]
#[case::authority("authority")]
#[case::write("semantic.write")]
#[case::read("semantic.read")]
#[case::source("source")]
#[case::point("point")]
#[case::digest("digest")]
#[case::ready("ready")]
#[case::history("history:READY")]
#[case::commit("commit")]
#[tokio::test]
async fn repository_failures_keep_the_original_failure_and_do_not_schedule_a_provider_retry(
	#[case] action: &str,
) {
	let f = Fixture::default();
	f.fault(action);
	assert!(
		matches!(process(&f,&f,f.id()).await,Err(Error::External(message)) if message==format!("fault:{action}"))
	);
	assert!(!f.has("failed"));
	assert!(!f.has("revoke_authority"));
}
#[rstest]
#[case::deleted(false, true, false)]
#[case::skip_locked(true, false, false)]
#[case::authority_changed(true, true, true)]
#[tokio::test]
async fn deleted_busy_or_rebound_entries_are_not_indexed(
	#[case] exists: bool,
	#[case] locked: bool,
	#[case] changed: bool,
) {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.exists = exists;
		s.locked = locked;
		if changed {
			s.current = json!({"changed":true});
		}
	}
	assert!(!process(&f, &f, f.id()).await.unwrap());
	assert!(!f.has("source"));
	assert!(!f.has("embed"));
	assert!(!f.has("revoked:source authority revoked or source removed"));
}
#[rstest]
#[case::workspace("workspace")]
#[case::write("write")]
#[case::read("read")]
#[case::source("source")]
#[case::disabled("disabled")]
#[tokio::test]
async fn unavailable_authority_source_or_index_retires_vectors_without_external_effects(
	#[case] reason: &str,
) {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		match reason {
			"workspace" => s.workspace_denied = true,
			"write" => s.write = false,
			"read" => s.read = false,
			"source" => s.text = None,
			"disabled" => s.plan.spec["enabled"] = json!(false),
			_ => panic!("unknown fixture"),
		}
	}
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert!(f.has("retire"));
	assert!(f.has("history:REVOKED"));
	assert!(f.has("commit"));
	assert!(!f.has("embed"));
	assert!(f.has(if reason == "disabled" {
		"revoked:semantic index disabled"
	} else {
		"revoked:source authority revoked or source removed"
	}));
	if reason == "workspace" {
		assert!(!f.has("semantic.write"));
	}
	if reason == "write" {
		assert!(!f.has("semantic.read"));
	}
}
#[rstest]
#[tokio::test]
async fn repeated_revocation_keeps_tombstones_without_duplicate_history() {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.text = None;
		s.entry.state = "REVOKED".into();
	}
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert!(f.has("retire"));
	assert!(!f.has("history:REVOKED"));
}
#[rstest]
#[tokio::test]
async fn an_acknowledged_current_point_is_deferred_without_embedding() {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.entry.state = "READY".into();
		s.old = Some((Some(indexing::content_digest("source text")), false));
		s.present = true;
	}
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert!(f.has("present"));
	assert!(f.has("defer"));
	assert!(!f.has("ensure"));
}
#[rstest]
#[case::retired(true, Some("old"))]
#[case::changed(false, Some("old"))]
#[case::retired_unwritten(true, None)]
#[tokio::test]
async fn changing_a_physical_point_commits_its_new_identity_before_external_io(
	#[case] retired: bool,
	#[case] digest: Option<&str>,
) {
	let f = Fixture::default();
	let original = f.0.lock().unwrap().entry.point_id;
	f.0.lock().unwrap().old = Some((digest.map(str::to_owned), retired));
	assert!(process(&f, &f, f.id()).await.unwrap());
	let next = f.0.lock().unwrap().next_point.unwrap();
	assert_ne!(original, next);
	assert!(!next.is_nil());
	assert!(f.has("schedule"));
	assert!(f.has("history:PENDING"));
	assert!(f.has("commit"));
	assert!(!f.has("ensure"));
	assert!(!f.has("embed"));
}
#[rstest]
#[case::ensure("ensure")]
#[case::embedding("embed")]
#[case::upsert("upsert")]
#[tokio::test]
async fn uncertain_external_requests_schedule_a_durable_bounded_retry(#[case] action: &str) {
	let f = Fixture::default();
	f.fault(action);
	f.0.lock().unwrap().entry.attempts = 8;
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert_eq!(f.0.lock().unwrap().retry, Some((9, 256.0)));
	assert!(f.has("history:ERROR"));
	assert!(f.has("commit"));
	assert!(!f.has("ready"));
}
#[rstest]
#[tokio::test]
async fn invalid_text_is_retried_without_contacting_an_external_backend() {
	let f = Fixture::default();
	f.0.lock().unwrap().text = Some("   ".into());
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert_eq!(f.0.lock().unwrap().retry, Some((1, 2.0)));
	assert!(!f.has("ensure"));
}
#[rstest]
#[tokio::test]
async fn a_present_check_failure_rewrites_the_current_point_under_the_same_authority() {
	let f = Fixture::default();
	f.fault("present");
	{
		let mut s = f.0.lock().unwrap();
		s.entry.state = "READY".into();
		s.old = Some((Some(indexing::content_digest("source text")), false));
	}
	assert!(process(&f, &f, f.id()).await.unwrap());
	assert!(f.has("upsert"));
	assert!(!f.has("rotate"));
}
#[rstest]
#[case::specification("spec")]
#[case::source("source")]
#[tokio::test]
async fn invalid_persisted_contracts_roll_back_before_provider_io(#[case] field: &str) {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		if field == "spec" {
			s.plan.spec["extra"] = json!(true);
		} else {
			s.entry.source = json!({"kind":"unknown"});
		}
	}
	assert!(process(&f, &f, f.id()).await.is_err());
	assert!(f.has("rollback"));
	assert!(!f.has("ensure"));
}
#[rstest]
#[tokio::test]
async fn stale_authority_is_checked_before_decoding_a_changed_index_specification() {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.plan.spec = json!({"invalid":true});
		s.current = json!({"changed":true});
	}
	assert!(!process(&f, &f, f.id()).await.unwrap());
	assert!(f.has("commit"));
}
#[rstest]
#[tokio::test]
async fn cancelled_embedding_rolls_back_the_restored_indexing_scope() {
	use std::{
		future::Future as _,
		task::{Context, Poll},
	};
	let f = Fixture::default();
	f.0.lock().unwrap().pending_embed = true;
	let id = f.id();
	let mut future = Box::pin(process(&f, &f, id));
	let waker = futures_util::task::noop_waker();
	let mut context = Context::from_waker(&waker);
	assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
	assert!(f.has("embed"));
	drop(future);
	assert!(f.has("rollback_drop"));
	assert!(!f.has("ready"));
	assert!(!f.has("failed"));
}
#[rstest]
#[case::acknowledged(false)]
#[case::uncertain(true)]
#[tokio::test]
async fn cleanup_acknowledgements_are_recorded_under_the_same_individual_lock(
	#[case] failure: bool,
) {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.points = vec![Uuid::from_u128(9)];
		s.collections = vec!["retired".into()];
	}
	if failure {
		f.fault("delete_point");
		f.fault("delete_collection");
	}
	cleanup(&f, &f).await.unwrap();
	assert_eq!(
		f.trace(),
		[
			"visibility",
			"cleanup_due",
			"visibility_drop",
			"visibility",
			"cleanup_begin",
			"lock_point",
			"delete_point",
			"record_point",
			"cleanup_commit",
			"visibility_drop",
			"visibility",
			"cleanup_begin",
			"lock_collection",
			"delete_collection",
			"record_collection",
			"cleanup_commit",
			"visibility_drop"
		]
	);
	assert_eq!(
		f.0.lock().unwrap().cleanup_failure,
		if failure {
			vec![
				Some("vector deletion failed; retry scheduled".into()),
				Some("collection deletion failed; retry scheduled".into()),
			]
		} else {
			vec![None, None]
		}
	);
}
#[rstest]
#[tokio::test]
async fn skip_locked_cleanup_does_not_delete_or_acknowledge_another_workers_identity() {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		s.points = vec![Uuid::from_u128(9)];
		s.collections = vec!["retired".into()];
		s.cleanup_locked = false;
	}
	cleanup(&f, &f).await.unwrap();
	assert!(!f.has("delete_point"));
	assert!(!f.has("delete_collection"));
	assert_eq!(
		f.trace()
			.iter()
			.filter(|a| a.as_str() == "cleanup_commit")
			.count(),
		2
	);
}
#[rstest]
#[case::point_lock("lock_point")]
#[case::point_record("record_point")]
#[case::collection_lock("lock_collection")]
#[case::collection_record("record_collection")]
#[case::commit("cleanup_commit")]
#[tokio::test]
async fn cleanup_database_failures_roll_back_the_unacknowledged_tombstone(#[case] action: &str) {
	let f = Fixture::default();
	{
		let mut s = f.0.lock().unwrap();
		if action.contains("collection") {
			s.collections = vec!["retired".into()];
		} else {
			s.points = vec![Uuid::from_u128(9)];
		}
	}
	f.fault(action);
	assert!(cleanup(&f, &f).await.is_err());
	assert!(f.has("cleanup_rollback"));
}
#[rstest]
#[tokio::test]
async fn sweep_releases_selection_visibility_and_reacquires_it_for_each_effect() {
	let f = Fixture::default();
	assert_eq!(sweep(&f, &f).await.unwrap(), 1);
	let trace = f.trace();
	assert_eq!(
		&trace[..5],
		[
			"visibility",
			"due",
			"visibility_drop",
			"visibility",
			"initial"
		]
	);
	assert_eq!(
		&trace[trace.len() - 4..],
		[
			"visibility_drop",
			"visibility",
			"cleanup_due",
			"visibility_drop"
		]
	);
}

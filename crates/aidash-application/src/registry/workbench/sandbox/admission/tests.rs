use super::*;
use crate::ports::{
	ModelProvider,
	registry::workbench::sandbox::{
		SandboxRepository, SandboxScope,
		admission::AdmissionScope,
		dispatch::{RealDispatchRepository, RealDispatchScope},
		execution::{ExecutionRepository, ExecutionScope},
	},
};
use aidash_domain::{
	identity::Principal,
	provider::ModelResponse,
	registry::{
		EntityRef, Entry,
		workbench::{
			Draft,
			profile::TestProfile,
			sandbox::{Fixture, FixtureStatus, TestLimits, TestOutcome},
		},
	},
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::Value;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct State {
	active: bool,
	limits_locked: bool,
	committed: bool,
	admitted: Option<Value>,
	order: Vec<&'static str>,
}
struct Repository {
	state: Mutex<State>,
	failure: Option<&'static str>,
	pause: bool,
}
impl Repository {
	fn new() -> Self {
		Self {
			state: Mutex::new(State::default()),
			failure: None,
			pause: false,
		}
	}
	fn admission(&self) -> Admission<'_> {
		Admission {
			repository: self,
			credentials: self,
			models: self,
		}
	}
}
struct Scope<'a> {
	repository: &'a Repository,
	pending: Option<Value>,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().active = false;
	}
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn entry(id: &str, kind: &str, config: Value) -> Entry {
	serde_json::from_value(
		json!({"id":id,"version":"1.0.0","kind":kind,"name":{},"description":{},"config":config}),
	)
	.unwrap()
}
fn agent() -> Entry {
	entry(
		"agent",
		"agent",
		json!({"schema_version":1,"model":reference("model"),"instructions":"private instructions","bindings":(0..3).map(|i| crate::test_support::binding("tool","aidash://fixture",&format!("tool_{i}"))).collect::<Vec<_>>(),"remove_default":aidash_domain::registry::bindings::DEFAULT_TOOLS,"max_steps":5}),
	)
}
fn draft() -> Draft {
	Draft {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		revision: 7,
		entry: json!(agent()),
		documents: json!([]),
		release_notes: String::new(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
	}
}
fn limits() -> TestLimits {
	TestLimits {
		tenant: "tenant".into(),
		max_input_bytes: 1_000_000,
		max_output_tokens: 128,
		max_total_tokens: 10000,
		max_steps: 3,
		max_duration_secs: 30,
		max_concurrent: 2,
		payload_days: 3,
		incident_evidence_days: 7,
	}
}
fn session() -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: draft().id,
		tenant: "tenant".into(),
		revision: 7,
		status: "running".into(),
		scenario: json!({}),
		conversation: Some(json!([])),
		tool_calls: Some(json!([])),
		usage: json!({}),
		error: None,
		created_at: Utc.timestamp_opt(100, 0).unwrap(),
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		expires_at: Utc.timestamp_opt(200, 0).unwrap(),
		expired_at: None,
	}
}
fn input() -> TestInput {
	TestInput {
		expected_revision: 7,
		message: "current".into(),
		mode: "simulated".into(),
		profile_id: None,
		continue_from: None,
		fixtures: BTreeMap::new(),
	}
}
fn rule() -> RealToolRule {
	RealToolRule {
		read_only_verified: true,
		tool: reference("tool_2"),
		endpoint: "https://test.example/rpc".into(),
		credential_env: Some("TEST_TOOL".into()),
		allowed_actions: vec!["read".into()],
		allowed_resources: vec!["fixture".into()],
	}
}
#[async_trait]
impl SandboxRepository for Repository {
	fn principal(&self) -> Principal {
		Principal::Subject {
			tenant: "tenant".into(),
			subject: "owner".into(),
		}
	}
	fn validate_limit_fields(&self, _: &TestLimits) -> Result<()> {
		self.state.lock().unwrap().order.push("field_validation");
		Ok(())
	}
	async fn begin(&self) -> Result<Box<dyn SandboxScope + '_>> {
		panic!("admission scope required")
	}
	async fn purge(&self) -> Result<u64> {
		panic!("admission uses atomic abandoned-slot expiry")
	}
}
#[async_trait]
impl RealDispatchRepository for Repository {
	async fn begin_real(&self) -> Result<Box<dyn RealDispatchScope + '_>> {
		panic!("admission must not dispatch")
	}
}
#[async_trait]
impl ExecutionRepository for Repository {
	async fn begin_execution(&self) -> Result<Box<dyn ExecutionScope + '_>> {
		panic!("admission scope required")
	}
	async fn read_session(&self, _: Uuid) -> Result<TestSession> {
		panic!("continuation must use the admission scope")
	}
	async fn progress(&self, _: Uuid, _: Value, _: Value, _: Value) -> Result<()> {
		panic!("admission must not execute")
	}
	async fn finish(&self, _: Uuid, _: &TestOutcome) -> Result<()> {
		panic!("admission must not finish")
	}
}
#[async_trait]
impl AdmissionRepository for Repository {
	async fn begin_admission(&self) -> Result<Box<dyn AdmissionScope + '_>> {
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.active = true;
		s.order.push("begin");
		Ok(Box::new(Scope {
			repository: self,
			pending: None,
		}))
	}
}
#[async_trait]
impl SandboxScope for Scope<'_> {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		assert_eq!(id, draft().id);
		assert!(lock);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("draft_update_lock");
		let mut row = draft();
		if self.repository.failure == Some("revision") {
			row.revision = 8;
		}
		row.archived = self.repository.failure == Some("archived");
		Ok(row)
	}
	async fn authorize_draft(&mut self, _: &Draft, action: &str, shares: bool) -> Result<()> {
		assert_eq!(action, "agent_draft.test");
		assert!(shares);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("current_authority");
		if self.repository.pause {
			std::future::pending::<()>().await;
		}
		if self.repository.failure == Some("authority") {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn limits(&mut self, tenant: &str) -> Result<TestLimits> {
		assert_eq!(tenant, "tenant");
		let mut s = self.repository.state.lock().unwrap();
		assert!(s.active);
		s.limits_locked = true;
		s.order.push("tenant_limits_update_lock");
		let mut value = limits();
		if self.repository.failure == Some("limits") {
			value.max_total_tokens = 64;
		}
		if self.repository.failure == Some("input_bytes") {
			value.max_input_bytes = 1024;
		}
		Ok(value)
	}
	async fn save_limits(&mut self, _: &TestLimits) -> Result<()> {
		panic!("admission must not change limits")
	}
	async fn session(&mut self, id: Uuid, lock: bool) -> Result<TestSession> {
		assert_eq!(id, Uuid::from_u128(8));
		assert!(!lock);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("continuation");
		let mut previous = session();
		previous.id = id;
		previous.status = "completed".into();
		previous.scenario = json!({"mode":"simulated","profile_id":null,"profile_revision":null});
		previous.conversation = Some(
			json!([{"role":"user","content":"prior"},{"role":"assistant","content":"prior answer"}]),
		);
		if self.repository.failure == Some("continuation") {
			previous.revision = 6;
		}
		Ok(previous)
	}
	async fn sessions(&mut self, _: Uuid) -> Result<Vec<TestSession>> {
		panic!("admission must not page sessions")
	}
	async fn stop(&mut self, _: Uuid) -> Result<TestSession> {
		panic!("admission must not stop sessions")
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		let mut s = self.repository.state.lock().unwrap();
		assert!(s.active);
		assert!(s.limits_locked);
		s.admitted = self.pending.take();
		s.committed = true;
		s.order.push("commit");
		Ok(())
	}
}
#[async_trait]
impl RealDispatchScope for Scope<'_> {
	async fn lock_identity(&mut self) -> Result<()> {
		panic!("canonical draft authority locks the admitted actor")
	}
	async fn profile(&mut self, tenant: &str, id: &str) -> Result<TestProfile> {
		assert_eq!((tenant, id), ("tenant", "profile"));
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("profile_share_lock");
		let mut rule = rule();
		if self.repository.failure == Some("profile_foreign") {
			rule.tool = reference("foreign");
		}
		Ok(TestProfile {
			tenant: tenant.into(),
			id: id.into(),
			revision: 4,
			enabled: self.repository.failure != Some("profile_disabled"),
			rules: if self.repository.failure == Some("profile_empty") {
				json!([])
			} else {
				json!([rule])
			},
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn effective(&mut self, selected: &EntityRef) -> Result<Entry> {
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push(if selected.id == "model" {
				"effective_model"
			} else {
				"effective_tool"
			});
		Ok(if selected.id == "model" {
			entry(
				"model",
				"model",
				json!({"provider":"fixture","model_id":"model","endpoint":"https://model.example/infer","credential_env":"MODEL_TEST","context_window":20000,"max_output_tokens":96,"modalities":["text"],"cost":{}}),
			)
		} else {
			entry(
				&selected.id,
				"tool",
				if selected.id == "tool_1" {
					json!({"transport":"agent","node_id":"node","agent":reference("delegate")})
				} else {
					json!({"transport":"http","endpoint":"https://production.example/rpc","credential_env":null,"replay":"read_only"})
				},
			)
		})
	}
	async fn write_calls(&mut self, _: Uuid, _: Value, _: bool) -> Result<()> {
		panic!("admission must not dispatch")
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		panic!("RAII owns admission rollback")
	}
}
#[async_trait]
impl ExecutionScope for Scope<'_> {
	async fn bindings(
		&mut self,
		_: &Draft,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::resolve(
			"aidash://fixture",
			entry,
			false,
			(0..3)
				.map(|i| {
					crate::test_support::http_tool(
						"aidash://fixture",
						&format!("tool_{i}"),
						&format!("plugin_{i}"),
					)
				})
				.collect(),
		))
	}

	async fn validate_content(&mut self, pinned: &Draft) -> Result<Entry> {
		assert_eq!(pinned.revision, 7);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("validate_dependencies");
		Ok(agent())
	}
}
#[async_trait]
impl AdmissionScope for Scope<'_> {
	async fn admit(
		&mut self,
		pinned: &Draft,
		configured: &TestLimits,
		scenario: Value,
		conversation: Value,
	) -> Result<TestSession> {
		{
			let mut s = self.repository.state.lock().unwrap();
			assert!(s.active);
			assert!(s.limits_locked);
			assert!(!s.committed);
			s.order.push("admit");
		}
		assert_eq!(pinned.id, draft().id);
		assert_eq!(configured.max_concurrent, 2);
		if self.repository.failure == Some("concurrency") {
			return Err(Error::Conflict(
				"tenant test concurrency limit reached".into(),
			));
		}
		self.pending = Some(json!({"scenario":scenario,"conversation":conversation}));
		let mut row = session();
		row.scenario = scenario;
		row.conversation = Some(conversation);
		Ok(row)
	}
}
impl Credentials for Repository {
	fn resolve(&self, name: &str) -> Result<String> {
		assert!(matches!(name, "MODEL_TEST" | "TEST_TOOL"));
		self.state
			.lock()
			.unwrap()
			.order
			.push(if name == "MODEL_TEST" {
				"model_fingerprint"
			} else {
				"tool_fingerprint"
			});
		Ok("fixture".into())
	}
}
struct NeverInfer;
#[async_trait]
impl ModelProvider for NeverInfer {
	async fn infer(&self, _: ModelRequest) -> Result<ModelResponse> {
		panic!("admission must not call inference")
	}
}
impl SandboxModels for Repository {
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>> {
		assert_eq!(model.model_id, "model");
		let mut s = self.state.lock().unwrap();
		assert!(s.active);
		assert!(s.limits_locked);
		assert!(!s.committed);
		s.order.push("model_factory");
		if self.failure == Some("provider") {
			Err(Error::Invalid("provider construction failed".into()))
		} else {
			Ok(Arc::new(NeverInfer))
		}
	}
}
#[tokio::test]
async fn admission_preserves_current_authority_locks_and_exact_plugin_indices() {
	// Arrange an agent with a disabled delegated Tool between two HTTP Tools.
	let repository = Repository::new();
	// Act without starting inference or a detached task.
	let admitted = admit(&repository.admission(), draft().id, input())
		.await
		.unwrap();
	// Assert commit precedes delivery, retained aliases use original indices, and capabilities remain confined.
	let state = repository.state.lock().unwrap();
	assert!(state.committed);
	assert!(!state.active);
	assert!(state.admitted.is_some());
	assert_eq!(
		state.order,
		[
			"begin",
			"draft_update_lock",
			"current_authority",
			"validate_dependencies",
			"effective_model",
			"model_fingerprint",
			"tenant_limits_update_lock",
			"field_validation",
			"model_factory",
			"admit",
			"commit"
		]
	);
	let names = admitted
		.job
		.request
		.tools
		.iter()
		.map(|tool| tool.name.as_str())
		.collect::<Vec<_>>();
	assert!(names.contains(&"plugin_0"));
	assert!(names.contains(&"plugin_2"));
	assert!(names.contains(&"plugin_1"));
	for excluded in [
		"task_create",
		"task_delegate",
		"task_assign",
		"memory_mutate",
		"memory_recall",
		"memory_reflect",
	] {
		assert!(!names.contains(&excluded));
	}
	assert_eq!(admitted.job.request.max_output_tokens, 96);
	assert_eq!(admitted.job.agent_max_steps, 5);
	assert_eq!(admitted.job.limits.max_steps, 3);
	assert_eq!(admitted.job.pinned_draft.revision, 7);
	assert_eq!(
		admitted.job.initial_conversation,
		vec![json!({"role":"user","content":"current"})]
	);
	assert_eq!(
		admitted.session.scenario,
		json!({"mode":"simulated","profile_id":null,"profile_revision":null,"continue_from":null,"fixtures":{},"binding_snapshot":crate::test_support::resolve("aidash://fixture",&agent(),false,(0..3).map(|i|crate::test_support::http_tool("aidash://fixture",&format!("tool_{i}"),&format!("plugin_{i}"))).collect())})
	);
	assert!(
		!admitted
			.job
			.request
			.context
			.as_object()
			.unwrap()
			.contains_key("conversation")
	);
}
#[tokio::test]
async fn real_profile_is_pinned_with_fingerprints_and_exact_scenario() {
	let repository = Repository::new();
	let mut input = input();
	input.mode = "real".into();
	input.profile_id = Some("profile".into());
	let admitted = admit(&repository.admission(), draft().id, input)
		.await
		.unwrap();
	let profile = admitted.job.profile.as_ref().unwrap();
	assert_eq!((profile.id.as_str(), profile.revision), ("profile", 4));
	assert_eq!(profile.rules[0].tool, reference("tool_2"));
	assert_eq!(
		profile.credential_fingerprints["TEST_TOOL"],
		dispatch::credential_fingerprint(&repository, "TEST_TOOL").unwrap()
	);
	assert_eq!(admitted.session.scenario["profile_revision"], 4);
	assert_eq!(admitted.session.scenario["mode"], "real");
	assert!(
		admitted
			.job
			.request
			.instructions
			.contains("selected test connection profile")
	);
}
#[tokio::test]
async fn continuation_reuses_only_the_matching_completed_conversation() {
	let repository = Repository::new();
	let mut input = input();
	input.continue_from = Some(Uuid::from_u128(8));
	let admitted = admit(&repository.admission(), draft().id, input)
		.await
		.unwrap();
	assert_eq!(
		admitted.job.initial_conversation,
		vec![
			json!({"role":"user","content":"prior"}),
			json!({"role":"assistant","content":"prior answer"}),
			json!({"role":"user","content":"current"})
		]
	);
	assert_eq!(
		admitted.job.request.context["conversation"],
		json!(admitted.job.initial_conversation)
	);
}
#[rstest]
#[case("message")]
#[case("mode")]
#[case("profile")]
#[case("fixtures")]
#[tokio::test]
async fn invalid_request_is_rejected_before_transaction(#[case] invalid: &str) {
	let repository = Repository::new();
	let mut input = input();
	match invalid {
		"message" => input.message = " ".into(),
		"mode" => input.mode = "unsupported".into(),
		"profile" => input.profile_id = Some("profile".into()),
		_ => {
			for index in 0..65 {
				input.fixtures.insert(
					index.to_string(),
					Fixture {
						status: FixtureStatus::Success,
						response: json!(null),
					},
				);
			}
		}
	}
	assert!(
		admit(&repository.admission(), draft().id, input)
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert_eq!(state.order, Vec::<&str>::new());
	assert!(!state.active);
}
#[rstest]
#[case("authority")]
#[case("revision")]
#[case("archived")]
#[case("limits")]
#[case("provider")]
#[case("concurrency")]
#[tokio::test]
async fn rejected_admission_releases_all_leases_and_returns_no_session(
	#[case] failure: &'static str,
) {
	let mut repository = Repository::new();
	repository.failure = Some(failure);
	assert!(
		admit(&repository.admission(), draft().id, input())
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert!(!state.active);
	assert!(!state.committed);
	assert!(state.admitted.is_none());
}
#[rstest]
#[case("profile_disabled")]
#[case("profile_empty")]
#[case("profile_foreign")]
#[case("profile_missing")]
#[tokio::test]
async fn missing_disabled_empty_or_foreign_profile_is_never_admitted(
	#[case] failure: &'static str,
) {
	let mut repository = Repository::new();
	repository.failure = Some(failure);
	let mut input = input();
	input.mode = "real".into();
	if failure != "profile_missing" {
		input.profile_id = Some("profile".into());
	}
	assert!(
		admit(&repository.admission(), draft().id, input)
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert!(!state.active);
	assert!(!state.committed);
	assert!(state.admitted.is_none());
	assert!(!state.order.contains(&"model_factory"));
}
#[tokio::test]
async fn stale_continuation_stops_before_model_construction_or_admission() {
	let mut repository = Repository::new();
	repository.failure = Some("continuation");
	let mut input = input();
	input.continue_from = Some(Uuid::from_u128(8));
	assert!(
		admit(&repository.admission(), draft().id, input)
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert!(!state.order.contains(&"model_factory"));
	assert!(!state.committed);
	assert!(!state.active);
}
#[tokio::test]
async fn fixture_payload_limit_is_checked_without_persisting_an_active_slot() {
	let repository = Repository::new();
	let mut input = input();
	input.fixtures.insert(
		"shell".into(),
		Fixture {
			status: FixtureStatus::Success,
			response: json!("x".repeat(65536)),
		},
	);
	let error = admit(&repository.admission(), draft().id, input)
		.await
		.err()
		.unwrap();
	assert_eq!(error.to_string(), "test fixtures exceed 64 KiB");
	let state = repository.state.lock().unwrap();
	assert!(state.order.contains(&"model_factory"));
	assert!(!state.order.contains(&"admit"));
	assert!(!state.committed);
	assert!(!state.active);
}
#[tokio::test]
async fn cancellation_during_authorization_releases_the_exclusive_draft_scope() {
	let mut repository = Repository::new();
	repository.pause = true;
	let admission = repository.admission();
	{
		let operation = admit(&admission, draft().id, input());
		tokio::pin!(operation);
		assert!(futures_util::poll!(&mut operation).is_pending());
		assert!(repository.state.lock().unwrap().active);
	}
	let state = repository.state.lock().unwrap();
	assert!(!state.active);
	assert!(!state.committed);
	assert!(state.admitted.is_none());
}

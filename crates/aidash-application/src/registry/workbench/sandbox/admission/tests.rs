use super::*;
use crate::ports::{
	ModelProvider,
	registry::workbench::sandbox::{
		SandboxRepository, SandboxScope,
		admission::AdmissionScope,
		dispatch::{RealDispatchRepository, RealDispatchScope},
		execution::{ExecutionRepository, ExecutionScope},
	},
	tools::PluginSessions,
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
use std::collections::BTreeMap;
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
	skill_source: Option<Value>,
	deferred: bool,
	plugins: Vec<crate::ports::bindings::StoredAgentPlugin>,
	/// The server settings choose eager exposure under `deferred@1`.
	eager: bool,
	/// What the previous session of a continuation stored in its scenario.
	previous_tools: Option<Value>,
	sessions: Option<Arc<dyn PluginSessions>>,
}
impl Repository {
	fn new() -> Self {
		Self {
			state: Mutex::new(State::default()),
			failure: None,
			pause: false,
			skill_source: None,
			deferred: false,
			plugins: vec![],
			eager: false,
			previous_tools: None,
			sessions: None,
		}
	}
	fn agent(&self) -> Entry {
		let mut entry = agent();
		if !self.plugins.is_empty() {
			entry.config["agent_plugins"] = json!(
				self.plugins
					.iter()
					.map(crate::test_support::plugin_reference)
					.collect::<Vec<_>>()
			);
			if self.eager {
				entry.config["agent_plugin_settings"] =
					json!({"acme":{"docs":{"exposure":"eager"}}});
			}
		}
		if self.skill_source.is_some() {
			entry.config["instructions"] = json!("");
			entry.config["bindings"] = json!([crate::test_support::binding(
				"source",
				"aidash://fixture",
				"mounted-source"
			)]);
			entry.config["remove_default"] = json!(
				aidash_domain::registry::bindings::DEFAULT_TOOLS
					.iter()
					.filter(|name| !aidash_domain::registry::bindings::SKILL_TOOLS.contains(name))
					.collect::<Vec<_>>()
			);
		}
		if self.deferred {
			let policy = aidash_domain::exposure::ExposurePolicy::Deferred(Default::default());
			entry.config["exposure"] = json!(policy);
			entry.config["remove_default"] = json!(
				policy
					.default_tools()
					.into_iter()
					.filter(|name| self.skill_source.is_none()
						|| *name != aidash_domain::registry::bindings::SKILL_ASSET_READ)
					.collect::<Vec<_>>()
			);
			let bindings = entry.config["bindings"].as_array_mut().unwrap();
			if self.skill_source.is_none() {
				bindings[0]["exposure"] = json!("eager");
			}
		}
		entry
	}
	fn admission(&self) -> Admission<'_> {
		Admission {
			repository: self,
			models: self,
			plugin_sessions: self.sessions.clone(),
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
		revision: 0,
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
		allowed_actions: vec!["read".into()],
		allowed_resources: vec!["fixture".into()],
		secret: None,
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
	fn ceilings(&self) -> aidash_domain::registry::workbench::sandbox::TestLimitCeilings {
		aidash_domain::registry::workbench::sandbox::TestLimitCeilings::MAXIMA
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
	async fn authorize_tenant(&mut self, _: &str, _: &str, _: &str) -> Result<()> {
		panic!("admission authorizes the draft")
	}
	async fn stored_limits(&mut self, _: &str) -> Result<TestLimits> {
		panic!("admission reads the limits that apply")
	}
	async fn save_limits(&mut self, _: &TestLimits) -> Result<TestLimits> {
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
		if let Some(tools) = &self.repository.previous_tools {
			previous.scenario["plugin_tools"] = tools.clone();
		}
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
				json!({"provider":"fixture","model_id":"model","endpoint":"https://model.example/infer","context_window":20000,"max_output_tokens":96,"modalities":["text"],"cost":{}}),
			)
		} else {
			entry(
				&selected.id,
				"tool",
				if selected.id == "tool_1" {
					json!({"transport":"agent","node_id":"node","agent":reference("delegate")})
				} else {
					json!({"transport":"http","endpoint":"https://production.example/rpc","replay":"read_only"})
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
		Ok(crate::test_support::resolve_with_plugins(
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
				.chain(self.repository.skill_source.iter().map(|source| {
					crate::test_support::entry("mounted-source", "source", source.clone())
				}))
				.collect(),
			self.repository.plugins.clone(),
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
		Ok(self.repository.agent())
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
struct NeverInfer;
#[async_trait]
impl ModelProvider for NeverInfer {
	async fn infer(
		&self,
		_: ModelRequest,
		_: &dyn crate::ports::InferenceProgressSink,
	) -> Result<ModelResponse> {
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
		json!({"mode":"simulated","profile_id":null,"profile_revision":null,"continue_from":null,"fixtures":{},"binding_snapshot":crate::test_support::resolve("aidash://fixture",&agent(),false,(0..3).map(|i|crate::test_support::http_tool("aidash://fixture",&format!("tool_{i}"),&format!("plugin_{i}"))).collect()),"plugin_tools":{"servers":[],"findings":[]}})
	);
	let aidash_domain::provider::ModelContext::Legacy(context) = &admitted.job.request.context
	else {
		panic!("Legacy request context");
	};
	assert!(!context.as_object().unwrap().contains_key("conversation"));
}
#[tokio::test]
async fn real_profile_is_pinned_with_the_exact_scenario() {
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
	let aidash_domain::provider::ModelContext::Legacy(context) = &admitted.job.request.context
	else {
		panic!("Legacy request context");
	};
	assert_eq!(
		context["conversation"],
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

#[rstest]
#[case::attachment(false)]
#[case::root(true)]
#[tokio::test]
async fn sole_skill_source_instructions_are_included_in_the_admitted_request(#[case] root: bool) {
	let mut repository = Repository::new();
	let instructions = "---\nname: fixture-skill\ndescription: skill source fixture\n---\nAlways inspect the selected workspace.";
	let attachment = aidash_domain::capabilities::skills::imported(
		format!("area:{}:.agents/skills/fixture", Uuid::from_u128(10)),
		instructions.into(),
		vec![],
	)
	.unwrap();
	let mut input = input();
	if root {
		repository.skill_source = Some(
			json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
		);
		let metadata = aidash_domain::capabilities::skills::validate(&attachment).unwrap();
		input.fixtures.insert(
			"skill_list".into(),
			Fixture {
				status: FixtureStatus::Success,
				response: json!({"skills":[metadata],"next_cursor":null,"truncated":false}),
			},
		);
		input.fixtures.insert(
			"skill_load".into(),
			Fixture {
				status: FixtureStatus::Success,
				response: json!({"skill":metadata,"path":"SKILL.md","content":instructions,"truncated":false}),
			},
		);
	} else {
		repository.skill_source = Some(
			json!({"schema_version":1,"source":{"adapter":"skill_attachments","attachments":[attachment]}}),
		);
	}
	let admitted = admit(&repository.admission(), draft().id, input)
		.await
		.unwrap();
	assert!(
		admitted
			.job
			.request
			.instructions
			.contains("Always inspect the selected workspace.")
	);
	assert!(admitted.job.request.instructions.contains("fixture-skill"));
	assert!(repository.state.lock().unwrap().committed);
}

#[tokio::test]
async fn unresolved_mounted_skill_context_cannot_produce_behavioral_evidence() {
	let mut repository = Repository::new();
	repository.skill_source = Some(
		json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
	);
	assert!(
		matches!(admit(&repository.admission(), draft().id, input()).await, Err(Error::Invalid(message)) if message.contains("Skill Sources require"))
	);
	assert!(!repository.state.lock().unwrap().committed);
}

#[rstest]
#[case::tools(false)]
#[case::attachment(true)]
#[tokio::test]
async fn deferred_agents_simulate_the_selected_first_request(#[case] attachment: bool) {
	let mut repository = Repository::new();
	repository.deferred = true;
	let instructions = "---\nname: fixture-skill\ndescription: skill source fixture\n---\nAlways inspect the selected workspace.";
	if attachment {
		let attachment = aidash_domain::capabilities::skills::imported(
			"fixture".into(),
			instructions.into(),
			vec![],
		)
		.unwrap();
		repository.skill_source = Some(
			json!({"schema_version":1,"source":{"adapter":"skill_attachments","attachments":[attachment]}}),
		);
	}
	let admitted = admit(&repository.admission(), draft().id, input())
		.await
		.unwrap();
	let request = &admitted.job.request;
	let names = request
		.tools
		.iter()
		.map(|tool| tool.name.as_str())
		.collect::<std::collections::BTreeSet<_>>();
	for mandatory in aidash_domain::registry::bindings::REQUIRED_TOOLS
		.iter()
		.chain(aidash_domain::registry::bindings::EXPOSURE_TOOLS)
	{
		assert!(names.contains(mandatory), "{mandatory}");
	}
	assert!(request.instructions.contains("capability_search"));
	if attachment {
		assert!(names.contains(aidash_domain::registry::bindings::SKILL_ASSET_READ));
		// A direct Skill is Discoverable: indexed, never resident until loaded.
		assert!(request.instructions.contains("skill_fixture_skill_"));
		assert!(
			!request
				.instructions
				.contains("Always inspect the selected workspace.")
		);
	} else {
		assert!(names.contains("plugin_0"));
		assert!(!names.contains("plugin_1") && !names.contains("plugin_2"));
		assert!(request.instructions.contains("plugin_1 [tool]"));
	}
	assert!(repository.state.lock().unwrap().committed);
}

const PLUGIN_TOOL: &str = "acme__docs__search";

fn servers(names: &[&str]) -> Value {
	names
		.iter()
		.map(|name| {
			(
				(*name).to_owned(),
				json!({"type":"streamable-http","url":format!("https://{name}.example.com/mcp")}),
			)
		})
		.collect::<serde_json::Map<_, _>>()
		.into()
}

/// A repository whose Agent takes in the plugin `acme` with the remote servers `names`, listed
/// over `sessions`.
fn plugin_repository(
	names: &[&str],
	sessions: &Arc<crate::test_support::FakeSessions>,
) -> Repository {
	let mut repository = Repository::new();
	repository.plugins = vec![crate::test_support::stored_plugin_with_servers(
		"acme",
		servers(names),
		&[],
	)];
	let sessions: Arc<dyn PluginSessions> = sessions.clone();
	repository.sessions = Some(sessions);
	repository
}

fn serving(tool: &str) -> Arc<crate::test_support::FakeSessions> {
	let sessions = Arc::new(crate::test_support::FakeSessions::default());
	sessions.script(
		"docs",
		crate::test_support::Listed::Tools(vec![crate::test_support::listed_tool(tool)]),
	);
	sessions
}

fn offered(request: &ModelRequest, alias: &str) -> bool {
	request.tools.iter().any(|tool| tool.name == alias)
}

#[rstest]
#[case::legacy(false, false)]
#[case::deferred_eager(true, true)]
#[case::deferred_discoverable(true, false)]
#[tokio::test]
async fn a_remote_plugin_server_is_listed_once_and_its_tools_are_offered(
	#[case] deferred: bool,
	#[case] eager: bool,
) {
	let sessions = serving("search");
	let mut repository = plugin_repository(&["docs"], &sessions);
	repository.deferred = deferred;
	repository.eager = eager;
	let admitted = admit(&repository.admission(), draft().id, input())
		.await
		.unwrap();
	let listings = sessions.listings();
	assert_eq!(listings.len(), 1);
	assert_eq!(listings[0].1.plugin.as_str(), "acme");
	assert_eq!(listings[0].1.server.as_str(), "docs");
	assert_eq!(sessions.closed(), [listings[0].0]);
	let request = &admitted.job.request;
	if deferred && !eager {
		assert!(!offered(request, PLUGIN_TOOL));
		assert!(
			request
				.instructions
				.contains(&format!("{PLUGIN_TOOL} [tool]"))
		);
	} else {
		assert!(offered(request, PLUGIN_TOOL));
	}
	assert!(!request.instructions.contains("unavailable in this test"));
	let stored = &admitted.session.scenario["plugin_tools"];
	assert_eq!(stored, &json!(admitted.job.plugin_tools));
	assert_eq!(stored["servers"][0]["tools"][0]["alias"], PLUGIN_TOOL);
	assert_eq!(stored["findings"], json!([]));
	assert!(repository.state.lock().unwrap().committed);
}

#[tokio::test]
async fn a_continued_session_reuses_the_pinned_tools_and_never_lists_again() {
	let sessions = serving("search");
	let first = plugin_repository(&["docs"], &sessions);
	let first = admit(&first.admission(), draft().id, input())
		.await
		.unwrap();
	// The server now lists a different tool, so a second listing would show.
	sessions.script(
		"docs",
		crate::test_support::Listed::Tools(vec![crate::test_support::listed_tool("other")]),
	);
	let mut second = plugin_repository(&["docs"], &sessions);
	second.previous_tools = Some(first.session.scenario["plugin_tools"].clone());
	let mut request = input();
	request.continue_from = Some(Uuid::from_u128(8));
	let continued = admit(&second.admission(), draft().id, request)
		.await
		.unwrap();
	assert_eq!(sessions.listings().len(), 1);
	assert_eq!(sessions.closed().len(), 1);
	assert!(offered(&continued.job.request, PLUGIN_TOOL));
	assert!(!offered(&continued.job.request, "acme__docs__other"));
	assert_eq!(
		continued.session.scenario["plugin_tools"],
		first.session.scenario["plugin_tools"]
	);
	assert_eq!(continued.job.plugin_tools, first.job.plugin_tools);
}

#[tokio::test]
async fn a_real_tool_test_offers_plugin_tools_without_referencing_them() {
	let sessions = serving("search");
	let repository = plugin_repository(&["docs"], &sessions);
	let mut request = input();
	request.mode = "real".into();
	request.profile_id = Some("profile".into());
	let admitted = admit(&repository.admission(), draft().id, request)
		.await
		.unwrap();
	assert!(offered(&admitted.job.request, PLUGIN_TOOL));
	assert!(!admitted.job.tool_references.contains_key(PLUGIN_TOOL));
	assert!(admitted.job.profile.is_some());
}

#[tokio::test]
async fn without_a_session_port_every_server_is_skipped_and_the_test_says_so() {
	let sessions = serving("search");
	let mut repository = plugin_repository(&["docs", "wiki"], &sessions);
	repository.sessions = None;
	let admitted = admit(&repository.admission(), draft().id, input())
		.await
		.unwrap();
	assert!(sessions.listings().is_empty());
	assert!(!offered(&admitted.job.request, PLUGIN_TOOL));
	assert!(admitted.job.plugin_tools.servers.is_empty());
	assert!(admitted.job.request.instructions.contains(
		"the tools of the plugin servers acme/docs, acme/wiki are unavailable in this test"
	));
	assert_eq!(
		skipped(&admitted.session.scenario["plugin_tools"]),
		["acme/docs", "acme/wiki"]
	);
}

#[rstest]
#[case::refused(None)]
#[case::unreachable(Some(crate::ports::tools::ListFailure::Unreachable))]
#[tokio::test]
async fn an_unavailable_server_is_skipped_while_the_others_are_offered(
	#[case] failure: Option<crate::ports::tools::ListFailure>,
) {
	let sessions = serving("search");
	if let Some(failure) = failure {
		sessions.script("wiki", crate::test_support::Listed::Failed(failure));
	}
	let repository = plugin_repository(&["docs", "wiki"], &sessions);
	let admitted = admit(&repository.admission(), draft().id, input())
		.await
		.unwrap();
	assert_eq!(sessions.listings().len(), 2);
	assert_eq!(sessions.closed().len(), 1);
	let request = &admitted.job.request;
	assert!(offered(request, PLUGIN_TOOL));
	assert!(
		request
			.instructions
			.contains("the tools of the plugin servers acme/wiki are unavailable in this test")
	);
	assert!(!request.instructions.contains("acme/docs"));
	assert_eq!(
		skipped(&admitted.session.scenario["plugin_tools"]),
		["acme/wiki"]
	);
}

/// The `plugin/server` pairs a stored pin reports as unavailable.
fn skipped(stored: &Value) -> Vec<String> {
	stored["findings"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|entry| entry["finding"]["reason"] == "server_unavailable")
		.map(|entry| {
			assert_eq!(entry["finding"]["outcome"], "skipped");
			format!(
				"{}/{}",
				entry["plugin"].as_str().unwrap(),
				entry["finding"]["component"]["server"].as_str().unwrap()
			)
		})
		.collect()
}

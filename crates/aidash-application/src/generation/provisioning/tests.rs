use super::*;
use crate::{
	authorization::Snapshot,
	generation::test_support,
	ports::{
		Credentials,
		generation::{provisioning::*, publication::GenerationPublication},
		registry::CoreToolCatalog,
	},
};
use aidash_domain::{
	capabilities::CoreCapabilities, generation::policy::Policy, provider::ToolSpec,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
	time::Duration,
};
use uuid::Uuid;

struct NoSecrets;
impl Credentials for NoSecrets {
	fn resolve(&self, _: &str) -> Result<String> {
		Err(Error::External("unexpected secret request".into()))
	}
}
struct NoCore;
impl CoreToolCatalog for NoCore {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(NoSecrets), Arc::new(NoCore))
}

#[derive(Clone, Copy)]
enum Failure {
	Unauthorized,
	Forbidden,
	Invalid,
	Conflict,
	DomainInvalid,
	DomainConflict,
	Json,
	External,
}
impl Failure {
	fn error(self) -> Error {
		match self {
			Self::Unauthorized => Error::Unauthorized,
			Self::Forbidden => Error::Forbidden,
			Self::Invalid => Error::Invalid("rejected".into()),
			Self::Conflict => Error::Conflict("rejected".into()),
			Self::DomainInvalid => aidash_domain::Error::Invalid("rejected".into()).into(),
			Self::DomainConflict => aidash_domain::Error::Conflict("rejected".into()).into(),
			Self::Json => Error::Json(serde_json::from_str::<Value>("{").unwrap_err()),
			Self::External => Error::External("database unavailable".into()),
		}
	}
}
struct State {
	now: DateTime<Utc>,
	job: Request,
	scan: Vec<Request>,
	spec: Spec,
	snapshot: Snapshot,
	current_enabled: bool,
	visible: bool,
	subjects: Vec<String>,
	phase: Option<String>,
	completion_ready: bool,
	fail: Option<(&'static str, Failure)>,
	pause: Option<&'static str>,
	calls: Vec<String>,
	committed: Vec<String>,
	visibility: usize,
	released: usize,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);
#[fixture]
fn repository() -> Repository {
	let spec: Spec = serde_json::from_value(test_support::specification()).unwrap();
	let mut job = test_support::request(1);
	job.status = "QUEUED".into();
	job.expires_at = DateTime::from_timestamp(2000, 0).unwrap();
	let mut definition = spec.template.clone();
	definition.id = job.agent_id.clone();
	job.definition = json!(definition);
	Repository(Arc::new(Mutex::new(State {
		now: DateTime::from_timestamp(1500, 0).unwrap(),
		scan: vec![job.clone()],
		job,
		spec,
		snapshot: Snapshot {
			revision: 7,
			bundle: serde_json::from_value(
				json!({"tenant":"tenant","subjects":{"alice":{"kind":"user"}}}),
			)
			.unwrap(),
		},
		current_enabled: true,
		visible: true,
		subjects: vec![],
		phase: None,
		completion_ready: true,
		fail: None,
		pause: None,
		calls: vec![],
		committed: vec![],
		visibility: 0,
		released: 0,
	})))
}
async fn point(state: &Arc<Mutex<State>>, name: &str) -> Result<()> {
	let (failure, pause) = {
		let mut state = state.lock().unwrap();
		state.calls.push(name.to_owned());
		(
			state
				.fail
				.filter(|(at, _)| *at == name)
				.map(|(_, failure)| failure),
			state.pause == Some(name),
		)
	};
	if let Some(failure) = failure {
		return Err(failure.error());
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Session {
	state: Arc<Mutex<State>>,
	kind: &'static str,
	snapshot: Snapshot,
	effects: Vec<String>,
	transitioned: Option<String>,
}
impl Session {
	fn new(state: Arc<Mutex<State>>, kind: &'static str) -> Self {
		let snapshot = state.lock().unwrap().snapshot.clone();
		Self {
			state,
			kind,
			snapshot,
			effects: vec![],
			transitioned: None,
		}
	}
	async fn effect(&mut self, name: &str) -> Result<()> {
		point(&self.state, name).await?;
		self.effects.push(name.to_owned());
		Ok(())
	}
	async fn transition(&mut self, status: &str, actor: &str, reason: &str) -> Result<()> {
		assert_eq!(actor, "generation-service");
		self.effect(&format!("transition:{status}:{reason}"))
			.await?;
		self.transitioned = Some(status.to_owned());
		Ok(())
	}
	async fn finish(mut self: Box<Self>, result: Result<()>) -> Result<()> {
		if result.is_err() {
			self.state
				.lock()
				.unwrap()
				.calls
				.push(format!("{}_rollback", self.kind));
			return result;
		}
		point(&self.state, &format!("{}_commit", self.kind)).await?;
		let mut state = self.state.lock().unwrap();
		state.committed.append(&mut self.effects);
		if let Some(status) = self.transitioned.take() {
			state.job.status = status;
		}
		if self.kind == "activation" {
			state.snapshot = self.snapshot.clone();
		}
		Ok(())
	}
}
impl Drop for Session {
	fn drop(&mut self) {
		self.state.lock().unwrap().released += 1;
	}
}
struct Read(Arc<Mutex<State>>);
impl Drop for Read {
	fn drop(&mut self) {
		let mut state = self.0.lock().unwrap();
		state.visibility -= 1;
		state.calls.push("read_drop".into());
	}
}
#[async_trait]
impl GenerationProvisionRead for Read {
	async fn jobs(&mut self) -> Result<Vec<Request>> {
		point(&self.0, "scan").await?;
		Ok(self.0.lock().unwrap().scan.clone())
	}
	fn notify(&self) {
		let mut state = self.0.lock().unwrap();
		assert_eq!(
			state.visibility, 1,
			"notification must retain the visibility lease"
		);
		state.calls.push("notify".into());
	}
}
#[async_trait]
impl GenerationProvisioning for Repository {
	fn now(&self) -> DateTime<Utc> {
		self.0.lock().unwrap().now
	}
	async fn dispatch_remote(&self) -> Result<()> {
		point(&self.0, "dispatch").await
	}
	async fn reconcile_foreign(&self) -> Result<()> {
		point(&self.0, "foreign").await
	}
	async fn begin_read(&self) -> Result<Box<dyn GenerationProvisionRead>> {
		point(&self.0, "read_begin").await?;
		self.0.lock().unwrap().visibility += 1;
		Ok(Box::new(Read(self.0.clone())))
	}
	async fn begin_activation(
		&self,
		job: &Request,
	) -> Result<Box<dyn GenerationActivationSession>> {
		assert_eq!(job.credential_id, Uuid::from_u128(4));
		point(&self.0, "activation_begin").await?;
		Ok(Box::new(Session::new(self.0.clone(), "activation")))
	}
	async fn begin_terminal(&self, _: &Request) -> Result<Box<dyn GenerationTerminalSession>> {
		point(&self.0, "terminal_begin").await?;
		let session = Session::new(self.0.clone(), "terminal");
		point(&self.0, "terminal_authority").await?;
		Ok(Box::new(session))
	}
	async fn completion_ready(&self, _: &Request) -> Result<bool> {
		Ok(self.0.lock().unwrap().completion_ready)
	}
}

#[rstest]
#[tokio::test]
async fn completion_work_defers_retirement_without_extending_the_original_expiry(
	repository: Repository,
) {
	let job = {
		let mut state = repository.0.lock().unwrap();
		state.job.status = "ACTIVE".into();
		state.phase = Some("COMPLETED".into());
		state.completion_ready = false;
		state.job.clone()
	};
	terminal(&repository, &job, "FAILED", "completed run")
		.await
		.unwrap();
	assert_eq!(repository.0.lock().unwrap().job.status, "ACTIVE");
	assert!(repository.0.lock().unwrap().committed.is_empty());
	assert!(
		!repository
			.0
			.lock()
			.unwrap()
			.calls
			.iter()
			.any(|call| call == "terminal_begin")
	);
	// Expiry bypasses the drain even when a crashed claim or human review waits.
	repository.0.lock().unwrap().now = job.expires_at;
	terminal(&repository, &job, "EXPIRED", "lifetime elapsed")
		.await
		.unwrap();
	assert_eq!(repository.0.lock().unwrap().job.status, "COMPLETED");
}
#[async_trait]
impl GenerationPublication for Session {
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn snapshot(&self) -> &Snapshot {
		&self.snapshot
	}
	fn replace_snapshot(&mut self, snapshot: Snapshot) {
		self.state
			.lock()
			.unwrap()
			.calls
			.push("replace_snapshot".into());
		self.snapshot = snapshot;
	}
	async fn save_authority(&mut self, _: &Request) -> Result<()> {
		self.effect("authority").await
	}
	async fn register(&mut self, _: &Entry) -> Result<()> {
		self.effect("register").await
	}
	async fn approve(&mut self, _: &Request, _: &Entry) -> Result<()> {
		self.effect("approve").await
	}
	async fn catalog_history(&mut self, _: &Request, _: &Entry) -> Result<()> {
		self.effect("catalog_history").await
	}
}
#[async_trait]
impl GenerationActivationSession for Session {
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		Session::finish(self, result).await
	}
}
#[async_trait]
impl GenerationActivationScope for Session {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::resolve(
			"aidash://local",
			entry,
			false,
			vec![
				crate::test_support::http_tool("aidash://local", "tool", "lookup"),
				crate::test_support::entry("skill", "skill", json!({"instructions":"Skill"})),
				crate::test_support::entry(
					"cluster",
					"cluster",
					json!({"coordinator":{"id":entry.id,"version":entry.version}}),
				),
			],
		))
	}

	fn now(&self) -> DateTime<Utc> {
		self.state.lock().unwrap().now
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		let mut state = self.state.lock().unwrap();
		state.calls.push("subjects".into());
		state.subjects = subjects;
	}
	async fn load(&mut self, tenant: &str, id: Uuid) -> Result<Request> {
		point(&self.state, "activation_load").await?;
		let job = self.state.lock().unwrap().job.clone();
		assert_eq!((&job.tenant, job.id), (&tenant.to_owned(), id));
		Ok(job)
	}
	async fn visible(&mut self, _: &Request) -> Result<bool> {
		point(&self.state, "visible").await?;
		Ok(self.state.lock().unwrap().visible)
	}
	async fn require_request(&mut self, _: &Request) -> Result<()> {
		point(&self.state, "generation.request").await
	}
	async fn current_policy(&mut self, job: &Request) -> Result<Policy> {
		point(&self.state, "current_policy").await?;
		let state = self.state.lock().unwrap();
		let mut spec = state.spec.clone();
		spec.enabled = state.current_enabled;
		// This revision must never replace the permissions/specification pinned by the request.
		spec.template.config["model"]["id"] = json!("changed-current-model");
		Ok(Policy {
			tenant: job.tenant.clone(),
			id: job.policy_id.clone(),
			revision: 99,
			spec,
			generated_count: 1,
			allocated_tokens: 10000,
			allocated_compaction_calls: 0,
			allocated_embedding_calls: 0,
		})
	}
	async fn pinned_policy(&mut self, job: &Request) -> Result<Value> {
		point(&self.state, "pinned_policy").await?;
		assert_eq!(job.policy_revision, 7);
		Ok(json!(self.state.lock().unwrap().spec))
	}
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		point(&self.state, &format!("catalog:{}:{action}", reference.id)).await?;
		Ok(self.state.lock().unwrap().spec.template.clone())
	}
	async fn transition(
		&mut self,
		_: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()> {
		Session::transition(self, status, actor, reason).await
	}
	async fn delegate(&mut self, task: Uuid, reference: &EntityRef) -> Result<()> {
		assert_eq!(task, Uuid::from_u128(2));
		assert_eq!(reference.id, "agent");
		self.effect("delegate").await
	}
}
#[async_trait]
impl GenerationTerminalSession for Session {
	async fn load(&mut self, _: &str, _: Uuid) -> Result<Request> {
		point(&self.state, "terminal_load").await?;
		Ok(self.state.lock().unwrap().job.clone())
	}
	async fn run_phase(&mut self, _: &Request) -> Result<Option<String>> {
		point(&self.state, "run_phase").await?;
		Ok(self.state.lock().unwrap().phase.clone())
	}
	async fn transition(
		&mut self,
		_: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()> {
		Session::transition(self, status, actor, reason).await
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		Session::finish(self, result).await
	}
}

#[rstest]
#[tokio::test]
async fn activation_rechecks_authority_and_publishes_the_pinned_spec_in_one_scope(
	repository: Repository,
	validation: DefinitionValidation,
) {
	// Arrange
	let job = repository.0.lock().unwrap().job.clone();
	// Act
	activate(&repository, &job, &validation).await.unwrap();
	// Assert
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state
			.calls
			.iter()
			.filter(|call| !call.starts_with("catalog:aidash."))
			.map(String::as_str)
			.collect::<Vec<_>>(),
		[
			"activation_begin",
			"activation_load",
			"subjects",
			"visible",
			"generation.request",
			"current_policy",
			"pinned_policy",
			"catalog:model:registry.read",
			"catalog:model:model.infer",
			"replace_snapshot",
			"authority",
			"register",
			"approve",
			"catalog_history",
			"transition:ACTIVE:registered approved definition",
			"delegate",
			"activation_commit"
		]
	);
	assert_eq!(state.subjects, job.subject_chain);
	assert_eq!(state.job.status, "ACTIVE");
	assert_eq!(state.snapshot.revision, 8);
	assert_eq!(state.released, 1);
}

#[rstest]
#[tokio::test]
async fn activation_checks_read_and_use_authority_for_every_pinned_component(
	repository: Repository,
	validation: DefinitionValidation,
) {
	// Arrange: each optional component must receive both current catalog checks.
	{
		let mut state = repository.0.lock().unwrap();
		state.spec.template.config["bindings"] = json!([
			crate::test_support::binding("tool", "aidash://local", "tool"),
			crate::test_support::binding("skill", "aidash://local", "skill")
		]);
		state.spec.template.config["cluster"] = json!({"id":"cluster","version":"1.0.0"});
		state.spec.compaction = Some(aidash_domain::generation::policy::Compaction {
			provider: EntityRef {
				id: "compact".into(),
				version: "1.0.0".into(),
			},
			calls_per_agent: 1,
			call_budget: 2,
		});
		state.spec.embedding = Some(aidash_domain::generation::policy::Embedding {
			provider: EntityRef {
				id: "embed".into(),
				version: "1.0.0".into(),
			},
			calls_per_agent: 1,
			call_budget: 2,
		});
	}
	let job = repository.0.lock().unwrap().job.clone();
	// Act
	activate(&repository, &job, &validation).await.unwrap();
	// Assert
	let state = repository.0.lock().unwrap();
	let calls: Vec<&str> = state
		.calls
		.iter()
		.filter(|call| call.starts_with("catalog:") && !call.starts_with("catalog:aidash."))
		.map(String::as_str)
		.collect();
	assert_eq!(
		calls,
		[
			"catalog:cluster:registry.read",
			"catalog:cluster:cluster.execute",
			"catalog:model:registry.read",
			"catalog:model:model.infer",
			"catalog:skill:registry.read",
			"catalog:skill:skill.use",
			"catalog:tool:registry.read",
			"catalog:tool:tool.invoke",
			"catalog:compact:registry.read",
			"catalog:compact:compaction.invoke",
			"catalog:embed:registry.read",
			"catalog:embed:embedding.invoke"
		]
	);
}

#[rstest]
#[case::already_active("ACTIVE", 2000, true, true, None)]
#[case::expired("QUEUED", 1500, true, true, Some("generation request expired"))]
#[case::hidden("QUEUED", 2000, false, true, Some("generated agent admission denied"))]
#[case::disabled("QUEUED", 2000, true, false, Some("generated agent admission denied"))]
#[tokio::test]
async fn stale_scan_state_is_reloaded_before_any_publication(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] status: &str,
	#[case] expiry: i64,
	#[case] visible: bool,
	#[case] enabled: bool,
	#[case] error: Option<&str>,
) {
	let job = repository.0.lock().unwrap().job.clone();
	{
		let mut state = repository.0.lock().unwrap();
		state.job.status = status.into();
		state.job.expires_at = DateTime::from_timestamp(expiry, 0).unwrap();
		state.visible = visible;
		state.current_enabled = enabled;
	}
	let result = activate(&repository, &job, &validation).await;
	match error {
		Some(expected) => assert_eq!(result.unwrap_err().to_string(), expected),
		None => result.unwrap(),
	}
	let state = repository.0.lock().unwrap();
	assert!(state.committed.is_empty());
	assert_eq!(state.snapshot.revision, 7);
	assert_eq!(state.released, 1);
	assert!(
		!state
			.calls
			.iter()
			.any(|call| call == "pinned_policy" || call == "register")
	);
}

#[rstest]
#[case("generation.request")]
#[case("catalog:model:registry.read")]
#[case("catalog:model:model.infer")]
#[tokio::test]
async fn current_denial_prevents_every_definition_authority_and_run_effect(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] at: &'static str,
) {
	let job = repository.0.lock().unwrap().job.clone();
	repository.0.lock().unwrap().fail = Some((at, Failure::Forbidden));
	let result = activate(&repository, &job, &validation).await;
	assert_eq!(
		result.unwrap_err().to_string(),
		"generated agent admission denied"
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.snapshot.revision, 7);
	assert!(state.committed.is_empty());
	assert!(!state.calls.iter().any(|call| call == "authority"));
	assert_eq!(state.released, 1);
}

#[rstest]
#[case("authority")]
#[case("register")]
#[case("approve")]
#[case("catalog_history")]
#[case("transition:ACTIVE:registered approved definition")]
#[case("delegate")]
#[case("activation_commit")]
#[tokio::test]
async fn late_failure_rolls_back_publication_transition_and_delegation(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] at: &'static str,
) {
	let job = repository.0.lock().unwrap().job.clone();
	repository.0.lock().unwrap().fail = Some((at, Failure::External));
	let result = activate(&repository, &job, &validation).await;
	assert_eq!(result.unwrap_err().to_string(), "database unavailable");
	let state = repository.0.lock().unwrap();
	assert_eq!(state.job.status, "QUEUED");
	assert_eq!(state.snapshot.revision, 7);
	assert!(state.committed.is_empty());
	assert_eq!(state.released, 1);
}

#[rstest]
#[case("register")]
#[case("delegate")]
#[tokio::test]
async fn cancellation_releases_the_owned_activation_and_discards_staged_effects(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] at: &'static str,
) {
	let job = repository.0.lock().unwrap().job.clone();
	repository.0.lock().unwrap().pause = Some(at);
	let result = tokio::time::timeout(
		Duration::from_millis(10),
		activate(&repository, &job, &validation),
	)
	.await;
	assert!(result.is_err());
	let state = repository.0.lock().unwrap();
	assert!(state.calls.iter().any(|call| call == at));
	assert_eq!(state.released, 1);
	assert!(state.committed.is_empty());
	assert_eq!(state.snapshot.revision, 7);
}

#[rstest]
#[case(Some("COMPLETED"), "COMPLETED", "run reached terminal state")]
#[case(Some("FAILED"), "FAILED", "run reached terminal state")]
#[case(Some("CANCELLED"), "STOPPED", "run reached terminal state")]
#[case(Some("READY"), "EXPIRED", "generation lifetime elapsed")]
#[case(None, "EXPIRED", "generation lifetime elapsed")]
#[tokio::test]
async fn terminal_rechecks_run_completion_after_the_exclusive_authority_lock(
	repository: Repository,
	#[case] phase: Option<&str>,
	#[case] expected: &str,
	#[case] reason: &str,
) {
	let job = repository.0.lock().unwrap().job.clone();
	repository.0.lock().unwrap().phase = phase.map(str::to_owned);
	terminal(&repository, &job, "EXPIRED", "generation lifetime elapsed")
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		[
			"terminal_begin",
			"terminal_authority",
			"terminal_load",
			"run_phase",
			&format!("transition:{expected}:{reason}"),
			"terminal_commit"
		]
	);
	assert_eq!(state.job.status, expected);
	assert_eq!(state.released, 1);
}

#[rstest]
#[case("FAILED")]
#[case("STOPPED")]
#[case("COMPLETED")]
#[case("DELETED")]
#[tokio::test]
async fn already_terminal_request_is_not_released_or_retired_again(
	repository: Repository,
	#[case] status: &str,
) {
	let job = repository.0.lock().unwrap().job.clone();
	repository.0.lock().unwrap().job.status = status.into();
	terminal(&repository, &job, "EXPIRED", "generation lifetime elapsed")
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert!(state.committed.is_empty());
	assert!(!state.calls.iter().any(|call| call == "run_phase"));
	assert_eq!(state.job.status, status);
}

#[rstest]
#[case(Failure::Unauthorized, true)]
#[case(Failure::Forbidden, true)]
#[case(Failure::Invalid, true)]
#[case(Failure::Conflict, true)]
#[case(Failure::DomainInvalid, true)]
#[case(Failure::DomainConflict, true)]
#[case(Failure::Json, false)]
#[case(Failure::External, false)]
#[tokio::test]
async fn reconciliation_records_only_admission_failures_and_preserves_retry_errors(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] failure: Failure,
	#[case] terminal_failure: bool,
) {
	repository.0.lock().unwrap().fail = Some(("activation_begin", failure));
	let result = reconcile(&repository, &validation).await;
	let state = repository.0.lock().unwrap();
	if terminal_failure {
		assert_eq!(result.unwrap(), 1);
		assert_eq!(state.job.status, "FAILED");
		assert!(state.calls.iter().any(|call| call == "notify"));
	} else {
		assert!(result.is_err());
		assert_eq!(state.job.status, "QUEUED");
		assert!(state.committed.is_empty());
		assert!(
			!state
				.calls
				.iter()
				.any(|call| call == "terminal_begin" || call == "notify")
		);
	}
	assert_eq!(
		&state.calls[..4],
		["dispatch", "foreign", "read_begin", "scan"]
	);
	assert_eq!(state.calls.last().unwrap(), "read_drop");
	assert_eq!(state.visibility, 0);
}

#[rstest]
#[case("dispatch")]
#[case("foreign")]
#[case("scan")]
#[tokio::test]
async fn recovery_failure_releases_visibility_without_notifying_or_starting_local_work(
	repository: Repository,
	validation: DefinitionValidation,
	#[case] at: &'static str,
) {
	repository.0.lock().unwrap().fail = Some((at, Failure::External));
	assert_eq!(
		reconcile(&repository, &validation)
			.await
			.unwrap_err()
			.to_string(),
		"database unavailable"
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.visibility, 0);
	assert!(state.committed.is_empty());
	assert!(
		!state
			.calls
			.iter()
			.any(|call| call == "activation_begin" || call == "notify")
	);
}

#[rstest]
#[tokio::test]
async fn empty_scan_keeps_remote_order_and_never_notifies(
	repository: Repository,
	validation: DefinitionValidation,
) {
	repository.0.lock().unwrap().scan.clear();
	assert_eq!(reconcile(&repository, &validation).await.unwrap(), 0);
	assert_eq!(
		repository.0.lock().unwrap().calls,
		["dispatch", "foreign", "read_begin", "scan", "read_drop"]
	);
}

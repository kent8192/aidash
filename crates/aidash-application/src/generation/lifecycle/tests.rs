use super::*;
use crate::{
	authorization::Snapshot, generation::test_support,
	ports::generation::lifecycle::GenerationControlScope,
};
use aidash_domain::policy::{PolicyBundle, Subject, SubjectKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::rstest;
use serde_json::Value;
use std::{
	collections::BTreeSet,
	sync::{Arc, Mutex},
	time::Duration,
};

struct State {
	job: Request,
	authority: Snapshot,
	now: DateTime<Utc>,
	after_replay: Option<DateTime<Utc>>,
	replay: bool,
	visible: bool,
	allowed: bool,
	retirement: Option<i64>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	calls: Vec<String>,
	effects: Vec<String>,
	active: usize,
	commits: usize,
	rollbacks: usize,
	notifications: usize,
}
struct Repository {
	principal: Principal,
	state: Arc<Mutex<State>>,
}
impl Repository {
	fn new(subject: bool) -> Self {
		let job = test_support::request(1);
		let mut bundle: PolicyBundle = serde_json::from_value(json!({"tenant":"tenant"})).unwrap();
		bundle.subjects.insert(
			qualified_agent("aidash://local", &job.agent_id, &job.agent_version),
			Subject {
				kind: SubjectKind::Agent,
				groups: BTreeSet::new(),
				roles: BTreeSet::new(),
				attributes: json!({}),
				enabled: true,
				delegated_by: Some("alice".into()),
			},
		);
		Self {
			principal: if subject {
				Principal::Subject {
					tenant: "tenant".into(),
					subject: "alice".into(),
				}
			} else {
				Principal::Operator
			},
			state: Arc::new(Mutex::new(State {
				job,
				authority: Snapshot {
					revision: 7,
					bundle,
				},
				now: DateTime::from_timestamp(999, 0).unwrap(),
				after_replay: None,
				replay: false,
				visible: true,
				allowed: true,
				retirement: Some(37),
				failure: None,
				pause: None,
				calls: vec![],
				effects: vec![],
				active: 0,
				commits: 0,
				rollbacks: 0,
				notifications: 0,
			})),
		}
	}
	fn calls(&self) -> Vec<String> {
		self.state.lock().unwrap().calls.clone()
	}
}
struct Scope {
	state: Arc<Mutex<State>>,
	effects: Vec<String>,
	job: Option<Request>,
	authority: Option<Snapshot>,
	completed: bool,
}
impl Scope {
	async fn point(&self, operation: &'static str) -> Result<()> {
		let (failure, pause) = {
			let mut state = self.state.lock().unwrap();
			assert_eq!(state.active, 1);
			state.calls.push(operation.into());
			(state.failure, state.pause)
		};
		if failure == Some(operation) {
			return Err(Error::External(format!("{operation} failed")));
		}
		if pause == Some(operation) {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
	fn finish(&mut self, result: Result<Request>) -> Result<Request> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("finish".into());
		state.active -= 1;
		self.completed = true;
		if result.is_ok() && state.failure == Some("commit") {
			state.rollbacks += 1;
			return Err(Error::External("commit failed".into()));
		}
		if result.is_ok() {
			state.commits += 1;
			state.effects.append(&mut self.effects);
			if let Some(job) = self.job.take() {
				state.job = job;
			}
			if let Some(authority) = self.authority.take() {
				state.authority = authority;
			}
		} else {
			state.rollbacks += 1;
		}
		result
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.completed {
			let mut state = self.state.lock().unwrap();
			state.active -= 1;
			state.rollbacks += 1;
			state.calls.push("scope.drop".into());
		}
	}
}
#[async_trait]
impl GenerationControls for Repository {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn begin(&self, _tenant: &str) -> Result<Box<dyn GenerationControlScope>> {
		let mut state = self.state.lock().unwrap();
		state.calls.push("begin".into());
		state.active += 1;
		Ok(Box::new(Scope {
			state: self.state.clone(),
			effects: vec![],
			job: None,
			authority: None,
			completed: false,
		}))
	}
	fn notify(&self) {
		let mut state = self.state.lock().unwrap();
		assert_eq!(state.active, 0);
		state.calls.push("notify".into());
		state.notifications += 1;
	}
}
#[async_trait]
impl GenerationControlScope for Scope {
	async fn visible(&mut self, _job: &Request) -> Result<bool> {
		self.point("visible").await?;
		Ok(self.state.lock().unwrap().visible)
	}
	async fn decide(&mut self, _job: &Request, action: &str) -> Result<bool> {
		self.point("decide").await?;
		let mut state = self.state.lock().unwrap();
		state.calls.push(action.into());
		Ok(state.allowed)
	}
	async fn finish(mut self: Box<Self>, result: Result<Request>) -> Result<Request> {
		Scope::finish(self.as_mut(), result)
	}
}
#[async_trait]
impl GenerationLifecycleScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn now(&self) -> DateTime<Utc> {
		self.state.lock().unwrap().now
	}
	async fn unused(
		&mut self,
		_job: &Request,
	) -> Result<(i64, aidash_domain::generation::policy::Allowances)> {
		self.point("unused").await?;
		Ok((
			70,
			aidash_domain::generation::policy::Allowances {
				compaction_calls: 8,
				embedding_calls: 13,
				summary_calls: 5,
			},
		))
	}
	async fn release_policy(
		&mut self,
		_job: &Request,
		unused: i64,
		unused_calls: &aidash_domain::generation::policy::Allowances,
	) -> Result<()> {
		self.point("release").await?;
		assert_eq!(
			(
				unused,
				unused_calls.compaction_calls,
				unused_calls.embedding_calls,
				unused_calls.summary_calls
			),
			(70, 8, 13, 5)
		);
		self.effects.push("quota.release".into());
		Ok(())
	}
	async fn mark_quota_released(&mut self, _job: &Request) -> Result<()> {
		self.point("quota").await?;
		self.effects.push("quota.mark".into());
		Ok(())
	}
	async fn cancel_runs(&mut self, _job: &Request) -> Result<()> {
		self.point("cancel").await?;
		self.effects.push("runs.cancel".into());
		Ok(())
	}
	async fn authority(&mut self, _tenant: &str) -> Result<Snapshot> {
		self.point("authority").await?;
		Ok(self
			.authority
			.clone()
			.unwrap_or_else(|| self.state.lock().unwrap().authority.clone()))
	}
	async fn save_authority(
		&mut self,
		_job: &Request,
		snapshot: &Snapshot,
		actor: &str,
	) -> Result<()> {
		self.point("save_authority").await?;
		self.authority = Some(snapshot.clone());
		self.effects
			.push(format!("authority:{}:{actor}", snapshot.revision));
		Ok(())
	}
	async fn retire_catalog(&mut self, _job: &Request) -> Result<Option<i64>> {
		self.point("retire_catalog").await?;
		Ok(self.state.lock().unwrap().retirement)
	}
	async fn record_retirement(
		&mut self,
		_job: &Request,
		revision: i64,
		actor: &str,
	) -> Result<()> {
		self.point("retirement").await?;
		self.effects.push(format!("retirement:{revision}:{actor}"));
		Ok(())
	}
	async fn update_status(&mut self, job: &Request, status: &str) -> Result<Request> {
		self.point("status").await?;
		let mut updated = job.clone();
		updated.status = status.into();
		updated.quota_released |= self.effects.contains(&"quota.mark".into());
		self.job = Some(updated.clone());
		self.effects.push(format!("status:{status}"));
		Ok(updated)
	}
	async fn history(
		&mut self,
		_job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()> {
		self.point("history").await?;
		self.effects
			.push(format!("history:{status}:{actor}:{reason}"));
		Ok(())
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.point("event").await?;
		assert_eq!(kind, "generation.changed");
		assert_eq!(workspace, self.state.lock().unwrap().job.workspace_id);
		assert_eq!(data["id"], json!(Uuid::from_u128(1)));
		self.effects.push("event".into());
		Ok(())
	}
	async fn replay(
		&mut self,
		_job: &Request,
		status: &str,
		actor: &str,
		input: &Control,
	) -> Result<bool> {
		self.point("replay").await?;
		let mut state = self.state.lock().unwrap();
		state
			.calls
			.push(format!("replay:{status}:{actor}:{}", input.reason));
		if let Some(now) = state.after_replay {
			state.now = now;
		}
		Ok(state.replay)
	}
	async fn load(&mut self, _tenant: &str, _id: Uuid) -> Result<Request> {
		self.point("load").await?;
		Ok(self
			.job
			.clone()
			.unwrap_or_else(|| self.state.lock().unwrap().job.clone()))
	}
}
fn stop() -> Control {
	Control {
		action: Action::Stop,
		reason: "operator request".into(),
	}
}

#[rstest]
#[case::approve(Action::Approve, "PENDING_APPROVAL", "QUEUED", "generation.approve")]
#[case::deny(Action::Deny, "PENDING_APPROVAL", "DENIED", "generation.approve")]
#[case::stop(Action::Stop, "ACTIVE", "STOPPED", "generation.stop")]
#[case::delete(Action::Delete, "COMPLETED", "DELETED", "generation.delete")]
#[tokio::test]
async fn authorized_control_preserves_transition_and_action_mapping(
	#[case] action: Action,
	#[case] previous: &str,
	#[case] expected: &str,
	#[case] permission: &str,
) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().job.status = previous.into();
	let job = control(
		&repo,
		"tenant",
		Uuid::from_u128(1),
		&Control {
			action,
			reason: "decision".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(job.status, expected);
	assert!(repo.calls().contains(&permission.into()));
	let calls = repo.calls();
	assert_eq!(&calls[calls.len() - 2..], ["finish", "notify"]);
	let state = repo.state.lock().unwrap();
	assert_eq!(
		(
			state.active,
			state.commits,
			state.rollbacks,
			state.notifications
		),
		(0, 1, 0, 1)
	);
	assert!(
		state
			.effects
			.contains(&format!("history:{expected}:alice:decision"))
	);
}
#[rstest]
#[tokio::test]
async fn operator_takes_the_policy_lock_before_job_load_and_notifies_only_after_commit() {
	let repo = Repository::new(false);
	control(&repo, "tenant", Uuid::from_u128(1), &stop())
		.await
		.unwrap();
	assert_eq!(&repo.calls()[..3], ["begin", "authority", "load"]);
	assert!(!repo.calls().contains(&"visible".into()));
	assert!(!repo.calls().contains(&"decide".into()));
	assert_eq!(
		repo.state.lock().unwrap().effects,
		vec![
			"quota.release",
			"quota.mark",
			"runs.cancel",
			"authority:8:operator",
			"retirement:37:operator",
			"status:STOPPED",
			"history:STOPPED:operator:operator request",
			"event"
		]
	);
}
#[rstest]
#[tokio::test]
async fn cross_tenant_subject_cannot_load_or_mutate_a_request() {
	let repo = Repository::new(true);
	assert!(matches!(
		control(&repo, "other", Uuid::from_u128(1), &stop()).await,
		Err(Error::Forbidden)
	));
	assert!(repo.calls().is_empty());
}
#[rstest]
#[case::visibility(false, true)]
#[case::control(true, false)]
#[tokio::test]
async fn current_subject_denial_precedes_saved_replay_and_every_mutation(
	#[case] visible: bool,
	#[case] allowed: bool,
) {
	let repo = Repository::new(true);
	{
		let mut state = repo.state.lock().unwrap();
		state.visible = visible;
		state.allowed = allowed;
		state.replay = true;
	}
	assert!(matches!(
		control(&repo, "tenant", Uuid::from_u128(1), &stop()).await,
		Err(Error::Forbidden)
	));
	let state = repo.state.lock().unwrap();
	assert!(!state.calls.contains(&"replay".into()));
	assert!(state.effects.is_empty());
	assert_eq!(state.notifications, 0);
}
#[rstest]
#[case::approve(Action::Approve, "ACTIVE")]
#[case::deny(Action::Deny, "QUEUED")]
#[case::stop(Action::Stop, "COMPLETED")]
#[case::delete(Action::Delete, "ACTIVE")]
#[tokio::test]
async fn invalid_transition_keeps_conflict_category_without_effects(
	#[case] action: Action,
	#[case] previous: &str,
) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().job.status = previous.into();
	assert!(
		matches!(control(&repo,"tenant",Uuid::from_u128(1),&Control {action,reason:"decision".into()}).await,Err(Error::Conflict(message)) if message=="invalid generation state transition")
	);
	assert!(repo.state.lock().unwrap().effects.is_empty());
	assert!(!repo.calls().contains(&"status".into()));
}
#[rstest]
#[tokio::test]
async fn expiry_is_checked_after_replay_lookup_against_the_current_clock() {
	let repo = Repository::new(true);
	{
		let mut state = repo.state.lock().unwrap();
		state.job.status = "PENDING_APPROVAL".into();
		state.after_replay = Some(DateTime::from_timestamp(1000, 0).unwrap());
	}
	assert!(
		matches!(control(&repo,"tenant",Uuid::from_u128(1),&Control {action:Action::Approve,reason:"decision".into()}).await,Err(Error::Conflict(message)) if message=="generation request expired")
	);
	assert!(repo.state.lock().unwrap().effects.is_empty());
}
#[rstest]
#[tokio::test]
async fn exact_saved_replay_reloads_without_second_retirement_history_or_event() {
	let repo = Repository::new(true);
	{
		let mut state = repo.state.lock().unwrap();
		state.job.status = "STOPPED".into();
		state.job.quota_released = true;
		state.replay = true;
	}
	assert_eq!(
		control(&repo, "tenant", Uuid::from_u128(1), &stop())
			.await
			.unwrap()
			.status,
		"STOPPED"
	);
	let state = repo.state.lock().unwrap();
	assert!(state.effects.is_empty());
	assert_eq!(
		state.calls.iter().filter(|c| c.as_str() == "load").count(),
		2
	);
	assert_eq!(state.notifications, 1);
}
#[rstest]
#[tokio::test]
async fn same_status_without_exact_replay_keeps_its_distinct_conflict() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().job.status = "STOPPED".into();
	assert!(
		matches!(control(&repo,"tenant",Uuid::from_u128(1),&stop()).await,Err(Error::Conflict(message)) if message=="generation decision already recorded")
	);
	assert!(repo.state.lock().unwrap().effects.is_empty());
}
#[rstest]
#[case("save_authority")]
#[case("retirement")]
#[case("status")]
#[case("history")]
#[case("event")]
#[case("commit")]
#[tokio::test]
async fn later_failure_rolls_back_quota_authority_retirement_history_and_outbox(
	#[case] failure: &'static str,
) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().failure = Some(failure);
	assert!(
		matches!(control(&repo,"tenant",Uuid::from_u128(1),&stop()).await,Err(Error::External(message)) if message==format!("{failure} failed"))
	);
	let state = repo.state.lock().unwrap();
	assert!(state.effects.is_empty());
	assert_eq!(state.job.status, "ACTIVE");
	assert!(!state.job.quota_released);
	assert_eq!(state.authority.revision, 7);
	assert_eq!(
		(
			state.active,
			state.commits,
			state.rollbacks,
			state.notifications
		),
		(0, 0, 1, 0)
	);
}
#[rstest]
#[case("history")]
#[case("event")]
#[tokio::test]
async fn cancellation_discards_all_staged_lifecycle_effects(#[case] pause: &'static str) {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().pause = Some(pause);
	assert!(
		tokio::time::timeout(
			Duration::from_millis(10),
			control(&repo, "tenant", Uuid::from_u128(1), &stop())
		)
		.await
		.is_err()
	);
	let state = repo.state.lock().unwrap();
	assert!(state.effects.is_empty());
	assert_eq!(state.job.status, "ACTIVE");
	assert_eq!(state.authority.revision, 7);
	assert_eq!(
		(
			state.active,
			state.commits,
			state.rollbacks,
			state.notifications
		),
		(0, 0, 1, 0)
	);
}
#[rstest]
#[tokio::test]
async fn foreign_retirement_keeps_the_exact_catalog_proof_without_a_local_workspace_event() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().job.home_node = "aidash://home".into();
	let job = control(&repo, "tenant", Uuid::from_u128(1), &stop())
		.await
		.unwrap();
	assert!(job.quota_released);
	let state = repo.state.lock().unwrap();
	assert!(state.effects.contains(&"retirement:37:alice".into()));
	assert!(!state.effects.contains(&"event".into()));
	let subject = qualified_agent("aidash://local", &job.agent_id, &job.agent_version);
	assert!(!state.authority.bundle.subjects[&subject].enabled);
}
#[rstest]
#[tokio::test]
async fn already_retired_request_does_not_release_or_retire_a_second_time() {
	let repo = Repository::new(true);
	{
		let mut state = repo.state.lock().unwrap();
		state.job.status = "COMPLETED".into();
		state.job.quota_released = true;
		state.retirement = None;
		for subject in state.authority.bundle.subjects.values_mut() {
			subject.enabled = false;
		}
	}
	control(
		&repo,
		"tenant",
		Uuid::from_u128(1),
		&Control {
			action: Action::Delete,
			reason: "cleanup".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(
		repo.state.lock().unwrap().effects,
		vec![
			"runs.cancel",
			"status:DELETED",
			"history:DELETED:alice:cleanup",
			"event"
		]
	);
	assert!(!repo.calls().contains(&"unused".into()));
	assert!(!repo.calls().contains(&"save_authority".into()));
}
#[rstest]
#[tokio::test]
async fn exhausted_authority_revision_cannot_commit_prior_quota_changes() {
	let repo = Repository::new(true);
	repo.state.lock().unwrap().authority.revision = i64::MAX;
	assert!(
		matches!(control(&repo,"tenant",Uuid::from_u128(1),&stop()).await,Err(Error::Invalid(message)) if message=="authorization revision exhausted")
	);
	let state = repo.state.lock().unwrap();
	assert!(state.effects.is_empty());
	assert!(!state.calls.contains(&"retire_catalog".into()));
	assert_eq!(state.notifications, 0);
}

#[rstest]
#[case::empty(String::new(), true)]
#[case::whitespace(" \n".into(), true)]
#[case::oversized("a".repeat(4097), false)]
#[tokio::test]
async fn control_reason_validation_precedes_replay_lookup_and_protected_effects(
	#[case] reason: String,
	#[case] domain_error: bool,
) {
	let repo = Repository::new(true);
	let result = control(
		&repo,
		"tenant",
		Uuid::from_u128(1),
		&Control {
			action: Action::Stop,
			reason,
		},
	)
	.await;
	if domain_error {
		assert!(matches!(
			result,
			Err(Error::Domain(aidash_domain::Error::Invalid(_)))
		));
	} else {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="control reason exceeds 4096 bytes")
		);
	}
	let state = repo.state.lock().unwrap();
	assert!(!state.calls.contains(&"replay".into()));
	assert!(state.effects.is_empty());
	assert_eq!(state.notifications, 0);
}

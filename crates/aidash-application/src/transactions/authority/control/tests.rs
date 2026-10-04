use super::*;
use crate::ports::transactions::authority::{AuthorityRepository, ControlScope, SubmissionScope};
use aidash_domain::{
	RunMetadata, Task,
	policy::Resource,
	transactions::{Isolation, Mutation, Participant, authority::SourceAdmission},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[fixture]
fn origin() -> Origin {
	Origin {
		credential_id: Uuid::from_u128(4),
		tenant: "tenant".into(),
		subject: "owner".into(),
	}
}

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants: ["aidash://home", "aidash://peer"]
			.into_iter()
			.map(|node| Participant {
				node_id: node.into(),
				mutations: vec![Mutation::WorkspaceState {
					workspace_id: Uuid::from_u128(3),
					expected_revision: 0,
					state: json!({"ready":true}),
				}],
			})
			.collect(),
	}
}

fn status(manifest: &Manifest) -> Status {
	Status {
		id: manifest.id,
		digest: manifest.digest().unwrap(),
		manifest: json!(manifest),
		decision: None,
		visible: false,
		complete: false,
		last_error: None,
		created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
	}
}

fn principal(origin: &Origin) -> ExecutionPrincipal {
	ExecutionPrincipal {
		tenant: origin.tenant.clone(),
		subject: origin.subject.clone(),
		credential_id: origin.credential_id,
	}
}

struct State {
	log: Vec<String>,
	origin: Option<Origin>,
	binding: Option<Binding>,
	status: Option<Status>,
	denial: Option<String>,
	failure: Option<String>,
	pending: bool,
	proof: Option<Preflight>,
	local: Option<Origin>,
	subjects: Vec<String>,
	source_credentials: Vec<Uuid>,
	attempts: Vec<(Uuid, String)>,
	pause_remote: bool,
}

#[derive(Clone)]
struct Repository {
	node: String,
	state: Arc<Mutex<State>>,
	concurrent: Arc<AtomicUsize>,
	peak: Arc<AtomicUsize>,
	started: Arc<Notify>,
	release: Arc<Notify>,
}

impl Repository {
	fn new(origin: Origin) -> Self {
		Self {
			node: "aidash://home".into(),
			state: Arc::new(Mutex::new(State {
				log: vec![],
				origin: Some(origin),
				binding: None,
				status: None,
				denial: None,
				failure: None,
				pending: true,
				proof: None,
				local: None,
				subjects: vec!["owner".into()],
				source_credentials: vec![],
				attempts: vec![],
				pause_remote: false,
			})),
			concurrent: Arc::new(AtomicUsize::new(0)),
			peak: Arc::new(AtomicUsize::new(0)),
			started: Arc::new(Notify::new()),
			release: Arc::new(Notify::new()),
		}
	}
	fn event(&self, event: impl Into<String>) -> Result<()> {
		let event = event.into();
		let mut state = self.state.lock().unwrap();
		state.log.push(event.clone());
		if state.denial.as_ref() == Some(&event) {
			return Err(Error::Forbidden);
		}
		if state.failure.as_ref() == Some(&event) {
			return Err(Error::External(event));
		}
		Ok(())
	}
	fn log(&self) -> Vec<String> {
		self.state.lock().unwrap().log.clone()
	}
	fn control(&self, origin: &Origin) -> Control {
		Control {
			repository: self.clone(),
			identity: principal(origin),
			subjects: self.state.lock().unwrap().subjects.clone(),
			binding: None,
			attempt: None,
			finished: false,
		}
	}
}

struct Control {
	repository: Repository,
	identity: ExecutionPrincipal,
	subjects: Vec<String>,
	binding: Option<Binding>,
	attempt: Option<(Uuid, String)>,
	finished: bool,
}
impl Drop for Control {
	fn drop(&mut self) {
		if !self.finished {
			self.repository
				.state
				.lock()
				.unwrap()
				.log
				.push("drop:control".into());
		}
	}
}
struct Submission {
	repository: Repository,
	status: Option<Status>,
	binding: Option<Binding>,
	abort: bool,
	finished: bool,
}
impl Drop for Submission {
	fn drop(&mut self) {
		if !self.finished {
			self.repository
				.state
				.lock()
				.unwrap()
				.log
				.push("drop:submission".into());
		}
	}
}

#[async_trait]
impl TransactionAuthorityScope for Control {
	fn identity(&self) -> ExecutionPrincipal {
		self.identity.clone()
	}
	fn source_node(&self) -> Option<&str> {
		None
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.subjects = subjects;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		Resource {
			tenant: self.identity.tenant.clone(),
			kind: kind.into(),
			id: id.to_string(),
			attributes,
		}
	}
	fn qualified_resource(&self, kind: &str, node: &str, id: Uuid) -> Resource {
		Resource {
			tenant: self.identity.tenant.clone(),
			kind: kind.into(),
			id: format!("{node}:{id}"),
			attributes: json!({"node_id":node}),
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.repository
			.event(format!("require:{action}:{}", resource.kind))
	}
	async fn inherit_task(&mut self, _: Uuid) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn inherit_local_run(&mut self, _: &RunMetadata) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn source_admission(
		&mut self,
		_: &RunMetadata,
		_: &str,
	) -> Result<Option<SourceAdmission>> {
		Err(Error::Forbidden)
	}
	async fn run(&mut self, _: Uuid) -> Result<RunMetadata> {
		Err(Error::Forbidden)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		Ok(self.resource("workspace", id, json!({})))
	}
	async fn task(&mut self, _: Uuid) -> Result<Task> {
		Err(Error::Forbidden)
	}
	async fn task_resource(&mut self, _: &Task) -> Result<Resource> {
		Err(Error::Forbidden)
	}
	async fn artifact_creation_resource(&mut self, _: Uuid, _: &str) -> Result<Resource> {
		Err(Error::Forbidden)
	}
}

#[async_trait]
impl ControlScope for Control {
	fn authority(&mut self) -> &mut dyn TransactionAuthorityScope {
		self
	}
	fn audit(&mut self, enabled: bool) {
		self.repository
			.state
			.lock()
			.unwrap()
			.log
			.push(format!("audit:{enabled}"));
	}
	async fn gate(&mut self) -> Result<()> {
		self.repository.event("gate")
	}
	async fn status(&mut self, _: Uuid) -> Result<Status> {
		self.repository.event("status")?;
		self.repository
			.state
			.lock()
			.unwrap()
			.status
			.clone()
			.ok_or_else(|| Error::NotFound("transaction".into()))
	}
	async fn match_origin(&mut self, _: Uuid, origin: &Origin) -> Result<()> {
		self.repository.event("match-origin")?;
		if self.repository.state.lock().unwrap().origin.as_ref() != Some(origin) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn bind(&mut self, _: Uuid, binding: &Binding) -> Result<()> {
		self.repository.event("bind:control")?;
		if self
			.repository
			.state
			.lock()
			.unwrap()
			.binding
			.as_ref()
			.is_some_and(|stored| stored != binding)
		{
			return Err(Error::Conflict(
				"transaction authority binding is immutable".into(),
			));
		}
		self.binding = Some(binding.clone());
		Ok(())
	}
	async fn trusted(&mut self, _: &str) -> Result<()> {
		self.repository.event("trusted")
	}
	async fn insert_attempt(&mut self, id: Uuid, node: &str) -> Result<()> {
		self.repository.event("insert-attempt")?;
		self.attempt = Some((id, node.into()));
		Ok(())
	}
	async fn pending_attempt(&mut self, _: Uuid, _: &str) -> Result<bool> {
		self.repository.event("pending-attempt")?;
		Ok(self.repository.state.lock().unwrap().pending)
	}
	fn into_submission(mut self: Box<Self>) -> Result<Box<dyn SubmissionScope>> {
		self.repository.event("transfer")?;
		self.finished = true;
		Ok(Box::new(Submission {
			repository: self.repository.clone(),
			status: None,
			binding: self.binding.take(),
			abort: false,
			finished: false,
		}))
	}
	async fn finish(mut self: Box<Self>, result: Result<()>) -> Result<()> {
		self.finished = true;
		self.repository.event(if result.is_ok() {
			"commit:control"
		} else if matches!(&result, Err(Error::Forbidden)) {
			"rollback:denied"
		} else {
			"rollback:control"
		})?;
		if result.is_ok() {
			let mut state = self.repository.state.lock().unwrap();
			if let Some(binding) = self.binding.take() {
				state.binding = Some(binding);
			}
			if let Some(attempt) = self.attempt.take() {
				state.attempts.push(attempt);
			}
		}
		result
	}
}

#[async_trait]
impl SubmissionScope for Submission {
	async fn submit(&mut self, manifest: &Manifest, _: &Origin) -> Result<Status> {
		self.repository.event("submit")?;
		let stored = status(manifest);
		self.status = Some(stored.clone());
		Ok(stored)
	}
	async fn bind(&mut self, _: Uuid, binding: &Binding) -> Result<()> {
		self.repository.event("bind:submission")?;
		self.binding = Some(binding.clone());
		Ok(())
	}
	async fn abort(&mut self, _: Uuid) -> Result<()> {
		self.repository.event("abort")?;
		self.abort = true;
		Ok(())
	}
	async fn finish(mut self: Box<Self>, result: Result<()>) -> Result<()> {
		self.finished = true;
		self.repository.event(if result.is_ok() {
			"commit:submission"
		} else {
			"rollback:submission"
		})?;
		if result.is_ok() {
			let mut state = self.repository.state.lock().unwrap();
			if let Some(status) = self.status.take() {
				state.status = Some(status);
			}
			if let Some(binding) = self.binding.take() {
				state.binding = Some(binding);
			}
			if self.abort {
				state.status.as_mut().unwrap().decision = Some("ABORT".into());
			}
		}
		result
	}
}

struct RemoteLease(Arc<AtomicUsize>);
impl Drop for RemoteLease {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::SeqCst);
	}
}

#[async_trait]
impl AuthorityRepository for Repository {
	fn node_id(&self) -> &str {
		&self.node
	}
	fn validate(&self, manifest: &Manifest) -> Result<()> {
		manifest.validate_with::<Error>(|_| Ok(()))
	}
	async fn origin(&self, _: Uuid) -> Result<Option<Origin>> {
		Ok(self.state.lock().unwrap().origin.clone())
	}
	async fn binding(&self, _: Uuid) -> Result<Option<Binding>> {
		Ok(self.state.lock().unwrap().binding.clone())
	}
	async fn source(&self, origin: &Origin) -> Result<Box<dyn ControlScope>> {
		self.event("source")?;
		self.state
			.lock()
			.unwrap()
			.source_credentials
			.push(origin.credential_id);
		Ok(Box::new(self.control(origin)))
	}
	async fn mapped(&self, input: &Preflight) -> Result<Box<dyn ControlScope>> {
		self.event("mapped")?;
		let origin = self
			.state
			.lock()
			.unwrap()
			.local
			.clone()
			.unwrap_or_else(|| input.origin.clone());
		Ok(Box::new(self.control(&origin)))
	}
	async fn remote_preflight(&self, node: &str, _: &Preflight) -> Result<()> {
		self.event(format!("preflight:{node}"))?;
		let current = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
		let _lease = RemoteLease(self.concurrent.clone());
		self.peak.fetch_max(current, Ordering::SeqCst);
		self.started.notify_one();
		let pause = self.state.lock().unwrap().pause_remote;
		if pause {
			self.release.notified().await;
		} else {
			tokio::task::yield_now().await;
		}
		Ok(())
	}
	async fn remote_access(&self, node: &str, _: &Preflight) -> Result<()> {
		self.event(format!("access:{node}"))
	}
	async fn remote_ticket(&self, _: &str, _: Uuid) -> Result<Preflight> {
		self.event("remote-ticket")?;
		let state = self.state.lock().unwrap();
		Ok(state
			.proof
			.clone()
			.unwrap_or_else(|| state.binding.as_ref().unwrap().request.clone()))
	}
	async fn fault(&self, _: Uuid, point: &str) -> Result<()> {
		self.event(point)
	}
	fn wake(&self) {
		self.state.lock().unwrap().log.push("wake".into());
	}
}

#[rstest]
#[tokio::test]
async fn submission_holds_admission_until_remote_preflight_then_commits_before_waking(
	origin: Origin,
	manifest: Manifest,
) {
	// Arrange: admission starts without a coordinator record.
	let repository = Repository::new(origin.clone());
	// Act.
	let result = submit(&repository, &principal(&origin), &manifest)
		.await
		.unwrap();
	// Assert: no durable record or binding precedes successful remote admission.
	assert_eq!(result, status(&manifest));
	let state = repository.state.lock().unwrap();
	assert_eq!(state.status.as_ref(), Some(&result));
	assert_eq!(state.binding.as_ref().unwrap().subjects, vec!["owner"]);
	let calls: Vec<&str> = state
		.log
		.iter()
		.filter(|call| !call.starts_with("require:"))
		.map(String::as_str)
		.collect();
	assert_eq!(
		calls,
		[
			"source",
			"status",
			"gate",
			"transfer",
			"submit",
			"bind:submission",
			"preflight:aidash://peer",
			"coordinator.submit.before",
			"commit:submission",
			"coordinator.submit.after",
			"wake"
		]
	);
}

#[rstest]
#[case("require:transaction.submit:workspace", true, "rollback:denied")]
#[case("preflight:aidash://peer", false, "rollback:submission")]
#[case("commit:submission", false, "commit:submission")]
#[tokio::test]
async fn failed_submission_keeps_no_record_and_never_wakes(
	origin: Origin,
	manifest: Manifest,
	#[case] fault: &str,
	#[case] denial: bool,
	#[case] finish: &str,
) {
	let repository = Repository::new(origin.clone());
	{
		let mut state = repository.state.lock().unwrap();
		if denial {
			state.denial = Some(fault.into());
		} else {
			state.failure = Some(fault.into());
		}
	}
	let result = submit(&repository, &principal(&origin), &manifest).await;
	assert!(result.is_err());
	let state = repository.state.lock().unwrap();
	assert_eq!(state.status, None);
	assert_eq!(state.binding, None);
	assert_eq!(state.log.last().map(String::as_str), Some(finish));
	assert!(
		!state
			.log
			.iter()
			.any(|call| call == "wake" || call == "coordinator.submit.after")
	);
}

#[rstest]
#[tokio::test]
async fn replay_rechecks_current_permissions_and_never_sends_another_preflight(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	let existing = status(&manifest);
	repository.state.lock().unwrap().status = Some(existing.clone());
	assert_eq!(
		submit(&repository, &principal(&origin), &manifest)
			.await
			.unwrap(),
		existing
	);
	assert!(
		!repository
			.log()
			.iter()
			.any(|call| call == "transfer" || call.starts_with("preflight:") || call == "wake")
	);
	{
		let mut state = repository.state.lock().unwrap();
		state.log.clear();
		state.denial = Some("require:transaction.submit:workspace".into());
	}
	assert!(matches!(
		submit(&repository, &principal(&origin), &manifest).await,
		Err(Error::Forbidden)
	));
	assert!(!repository.log().iter().any(|call| call == "status"));
	assert_eq!(
		repository.log().last().map(String::as_str),
		Some("rollback:denied")
	);
}

#[rstest]
#[tokio::test]
async fn preflight_reservation_is_immutable_and_denials_do_not_replace_it(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	let input = request(&manifest, &origin, &repository.node).unwrap();
	preflight(&repository, &manifest.coordinator, &input)
		.await
		.unwrap();
	let reserved = repository.state.lock().unwrap().binding.clone().unwrap();
	repository
		.state
		.lock()
		.unwrap()
		.subjects
		.push("new-agent".into());
	assert!(matches!(
		preflight(&repository, &manifest.coordinator, &input).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(
		repository.state.lock().unwrap().binding.as_ref(),
		Some(&reserved)
	);
	repository.state.lock().unwrap().log.clear();
	assert!(matches!(
		preflight(&repository, "aidash://untrusted", &input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.log(), Vec::<String>::new());
}

#[rstest]
#[case(Some("ABORT"), true, false)]
#[case(None, false, true)]
#[tokio::test]
async fn ticket_requires_an_undecided_record_and_a_pending_attempt(
	origin: Origin,
	manifest: Manifest,
	#[case] decision: Option<&str>,
	#[case] pending: bool,
	#[case] queries_attempt: bool,
) {
	let repository = Repository::new(origin);
	let mut stored = status(&manifest);
	stored.decision = decision.map(str::to_owned);
	{
		let mut state = repository.state.lock().unwrap();
		state.status = Some(stored);
		state.pending = pending;
	}
	assert!(matches!(
		ticket(&repository, manifest.id, "aidash://peer").await,
		Err(Error::Forbidden)
	));
	let calls = repository.log();
	assert_eq!(
		calls.iter().any(|call| call == "pending-attempt"),
		queries_attempt
	);
	assert!(!calls.iter().any(|call| call.starts_with("require:")));
	assert_eq!(calls.last().map(String::as_str), Some("rollback:denied"));
}

#[rstest]
#[case("authority.issue.before", false, "drop:control")]
#[case("authority.issue.after", true, "authority.issue.after")]
#[tokio::test]
async fn issue_faults_preserve_only_committed_attempts(
	origin: Origin,
	manifest: Manifest,
	#[case] fault: &str,
	#[case] durable: bool,
	#[case] last: &str,
) {
	let repository = Repository::new(origin);
	repository.state.lock().unwrap().failure = Some(fault.into());
	assert!(matches!(
		issue(&repository, &manifest, "aidash://peer").await,
		Err(Error::External(_))
	));
	let state = repository.state.lock().unwrap();
	assert_eq!(
		state.attempts,
		if durable {
			vec![(manifest.id, "aidash://peer".into())]
		} else {
			vec![]
		}
	);
	assert_eq!(state.log.last().map(String::as_str), Some(last));
}

#[rstest]
#[tokio::test]
async fn operator_recovery_does_not_invent_subject_authority(origin: Origin, manifest: Manifest) {
	let repository = Repository::new(origin);
	repository.state.lock().unwrap().origin = None;
	issue(&repository, &manifest, "aidash://peer")
		.await
		.unwrap();
	assert_eq!(repository.log(), Vec::<String>::new());
	assert_eq!(
		prepare_admission(&repository, &manifest.coordinator, &manifest)
			.await
			.unwrap(),
		None
	);
	repository.state.lock().unwrap().origin = Some(Origin {
		credential_id: Uuid::nil(),
		tenant: "tenant".into(),
		subject: "owner".into(),
	});
	assert!(matches!(
		prepare_admission(&repository, &manifest.coordinator, &manifest).await,
		Err(Error::Forbidden)
	));
}

#[rstest]
#[tokio::test]
async fn immutable_owner_survives_rotation_but_reads_use_the_current_credential(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	let stored = status(&manifest);
	repository.state.lock().unwrap().status = Some(stored.clone());
	let mut identity = principal(&origin);
	identity.credential_id = Uuid::from_u128(99);
	assert_eq!(
		require_owner(&repository, &identity, manifest.id)
			.await
			.unwrap(),
		origin
	);
	manage(&repository, &identity, &stored, "transaction.read")
		.await
		.unwrap();
	let calls = repository.log();
	assert_eq!(
		repository.state.lock().unwrap().source_credentials,
		vec![identity.credential_id]
	);
	assert!(calls.iter().any(|call| call == "audit:false"));
	assert!(calls.iter().any(|call| call == "access:aidash://peer"));
	assert!(!calls.iter().any(|call| call == "abort"));
	identity.subject = "other".into();
	assert!(matches!(
		require_owner(&repository, &identity, manifest.id).await,
		Err(Error::NotFound(_))
	));
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn admission_requires_the_reserved_subject_chain_and_exact_remote_proof(
	origin: Origin,
	manifest: Manifest,
	#[case] changed_chain: bool,
) {
	let mut repository = Repository::new(origin.clone());
	repository.node = "aidash://peer".into();
	let bound = Binding {
		request: request(&manifest, &origin, &repository.node).unwrap(),
		local: origin.clone(),
		subjects: vec!["owner".into()],
	};
	{
		let mut state = repository.state.lock().unwrap();
		state.binding = Some(bound.clone());
		if changed_chain {
			state.subjects.push("new-agent".into());
		} else {
			let mut proof = bound.request.clone();
			proof.digest = "changed".into();
			state.proof = Some(proof);
		}
	}
	let prepared = prepare_admission(&repository, &manifest.coordinator, &manifest)
		.await
		.unwrap()
		.unwrap();
	let mut scope = repository.control(&origin);
	assert!(matches!(
		check_admission(&mut scope, &repository, &prepared, &manifest.coordinator).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.log().iter().any(|call| call == "remote-ticket"),
		!changed_chain
	);
	assert_eq!(
		repository.state.lock().unwrap().binding.as_ref(),
		Some(&bound)
	);
}

#[rstest]
#[tokio::test]
async fn read_access_rejects_current_local_identity_changes_before_policy_checks(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	let input = request(&manifest, &origin, &repository.node).unwrap();
	{
		let mut state = repository.state.lock().unwrap();
		state.binding = Some(Binding {
			request: input.clone(),
			local: origin.clone(),
			subjects: vec!["owner".into()],
		});
		let mut changed = origin;
		changed.credential_id = Uuid::from_u128(77);
		state.local = Some(changed);
	}
	assert!(matches!(
		read_access(&repository, &manifest.coordinator, &input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.log(), ["mapped", "audit:false", "drop:control"]);
}

#[rstest]
#[tokio::test]
async fn submission_fanout_is_bounded_to_eight_independent_participants(
	origin: Origin,
	mut manifest: Manifest,
) {
	let local = manifest.participants[0].clone();
	manifest.participants = vec![local.clone()];
	for index in 0..15 {
		manifest.participants.push(Participant {
			node_id: format!("aidash://peer{index:02}"),
			mutations: local.mutations.clone(),
		});
	}
	let repository = Repository::new(origin.clone());
	submit(&repository, &principal(&origin), &manifest)
		.await
		.unwrap();
	assert_eq!(repository.peak.load(Ordering::SeqCst), 8);
	assert_eq!(repository.concurrent.load(Ordering::SeqCst), 0);
	assert_eq!(
		repository
			.log()
			.iter()
			.filter(|call| call.starts_with("preflight:"))
			.count(),
		15
	);
}

#[rstest]
#[tokio::test]
async fn cancellation_drops_the_uncommitted_submission_and_all_inflight_requests(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	repository.state.lock().unwrap().pause_remote = true;
	let task_repository = repository.clone();
	let task =
		tokio::spawn(async move { submit(&task_repository, &principal(&origin), &manifest).await });
	repository.started.notified().await;
	task.abort();
	assert!(task.await.unwrap_err().is_cancelled());
	let state = repository.state.lock().unwrap();
	assert_eq!(state.status, None);
	assert_eq!(state.binding, None);
	assert_eq!(
		state.log.last().map(String::as_str),
		Some("drop:submission")
	);
	assert!(
		!state
			.log
			.iter()
			.any(|call| call.starts_with("commit:") || call == "wake")
	);
	assert_eq!(repository.concurrent.load(Ordering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn live_trust_revocation_blocks_ticket_even_when_an_attempt_remains_pending(
	origin: Origin,
	manifest: Manifest,
) {
	let repository = Repository::new(origin.clone());
	repository.state.lock().unwrap().status = Some(status(&manifest));
	assert_eq!(
		ticket(&repository, manifest.id, "aidash://peer")
			.await
			.unwrap(),
		request(&manifest, &origin, "aidash://peer").unwrap()
	);
	assert_eq!(
		&repository.log()[repository.log().len() - 2..],
		["commit:control", "authority.checked.after"]
	);
	{
		let mut state = repository.state.lock().unwrap();
		state.log.clear();
		state.denial = Some("trusted".into());
	}
	assert!(matches!(
		ticket(&repository, manifest.id, "aidash://peer").await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.log(), ["source", "trusted", "rollback:denied"]);
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn abort_requires_both_live_subject_permission_and_remote_read_access(
	origin: Origin,
	manifest: Manifest,
	#[case] denied_locally: bool,
) {
	let repository = Repository::new(origin.clone());
	let stored = status(&manifest);
	repository.state.lock().unwrap().status = Some(stored.clone());
	{
		let mut state = repository.state.lock().unwrap();
		if denied_locally {
			state.denial = Some("require:transaction.abort:transaction".into());
		} else {
			state.failure = Some("access:aidash://peer".into());
		}
	}
	assert!(
		manage(
			&repository,
			&principal(&origin),
			&stored,
			"transaction.abort"
		)
		.await
		.is_err()
	);
	assert_eq!(
		repository
			.state
			.lock()
			.unwrap()
			.status
			.as_ref()
			.unwrap()
			.decision,
		None
	);
	assert!(
		!repository
			.log()
			.iter()
			.any(|call| call == "abort" || call == "transfer")
	);
	{
		let mut state = repository.state.lock().unwrap();
		state.log.clear();
		state.denial = None;
		state.failure = None;
	}
	manage(
		&repository,
		&principal(&origin),
		&stored,
		"transaction.abort",
	)
	.await
	.unwrap();
	assert_eq!(
		repository
			.state
			.lock()
			.unwrap()
			.status
			.as_ref()
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	assert_eq!(
		&repository.log()[repository.log().len() - 3..],
		["abort", "commit:submission", "coordinator.abort.after"]
	);
}

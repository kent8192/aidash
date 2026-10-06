use super::*;
use crate::ports::transactions::coordination::RecoveryBatch;
use aidash_domain::transactions::{CoordinatorState, Isolation, Mutation, Participant};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::Mutex;
use tokio::sync::Notify;

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T01:00:00Z".parse().unwrap(),
		participants: vec![Participant {
			node_id: "aidash://home".into(),
			mutations: vec![Mutation::WorkspaceState {
				workspace_id: Uuid::from_u128(2),
				expected_revision: 0,
				state: json!({}),
			}],
		}],
	}
}

#[derive(Clone, Copy)]
enum Failure {
	Conflict,
	Invalid,
	NotFound,
	Forbidden,
	Unauthorized,
	Pending,
	Unknown,
}

impl Failure {
	fn error(self) -> Error {
		match self {
			Self::Conflict => Error::Conflict("rejected".into()),
			Self::Invalid => Error::Invalid("rejected".into()),
			Self::NotFound => Error::NotFound("rejected".into()),
			Self::Forbidden => Error::Forbidden,
			Self::Unauthorized => Error::Unauthorized,
			Self::Pending => Error::TransactionPending,
			Self::Unknown => Error::Port(Box::new(std::io::Error::other("outcome unknown"))),
		}
	}
}

struct State {
	status: Status,
	votes: Vec<Vote>,
	log: Vec<String>,
	transport_failure: Option<Failure>,
	issue_failure: bool,
	response: Option<LocalStatus>,
	scoped: bool,
	busy: bool,
	pause: bool,
	fail_event: Option<String>,
}

#[derive(Clone)]
struct Adapter {
	state: Arc<Mutex<State>>,
	started: Arc<Notify>,
	continue_send: Arc<Notify>,
}

impl Adapter {
	fn new(manifest: &Manifest) -> Self {
		Self {
			state: Arc::new(Mutex::new(State {
				status: Status {
					id: manifest.id,
					digest: manifest.digest().unwrap(),
					manifest: json!(manifest),
					decision: None,
					visible: false,
					complete: false,
					last_error: None,
					created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
				},
				votes: vec![Vote {
					node_id: "aidash://home".into(),
					phase: "PENDING".into(),
				}],
				log: vec![],
				transport_failure: None,
				issue_failure: false,
				response: None,
				scoped: false,
				busy: false,
				pause: false,
				fail_event: None,
			})),
			started: Arc::new(Notify::new()),
			continue_send: Arc::new(Notify::new()),
		}
	}
	fn event(&self, event: impl Into<String>) -> Result<()> {
		let event = event.into();
		let mut state = self.state.lock().unwrap();
		state.log.push(event.clone());
		if state.fail_event.as_ref() == Some(&event) {
			return Err(Error::External(event));
		}
		Ok(())
	}
	fn log(&self) -> Vec<String> {
		self.state.lock().unwrap().log.clone()
	}
	fn coordinator(&self) -> Coordinator {
		Coordinator::new(Arc::new(self.clone()), Arc::new(self.clone()))
	}
}

struct Scope {
	adapter: Adapter,
	connection: bool,
	released: bool,
}

#[async_trait]
impl RecoveryScope for Scope {
	async fn touch(&mut self) -> Result<()> {
		self.connection = true;
		self.adapter.event("touch")
	}
	async fn acknowledge(&mut self, node: &str, phase: &str) -> Result<()> {
		self.adapter.event(format!("ack:{node}:{phase}"))?;
		let mut state = self.adapter.state.lock().unwrap();
		state
			.votes
			.iter_mut()
			.find(|vote| vote.node_id == node)
			.unwrap()
			.phase = phase.into();
		state.status.last_error = None;
		Ok(())
	}
	async fn record_error(&mut self, error: &str) -> Result<()> {
		self.adapter.event("record_error")?;
		self.adapter.state.lock().unwrap().status.last_error = Some(error.into());
		Ok(())
	}
	async fn release(mut self: Box<Self>) -> Result<()> {
		self.released = true;
		if self.connection {
			self.adapter.event("connection.release")?;
		}
		self.adapter.event("lease.release")
	}
}

impl Drop for Scope {
	fn drop(&mut self) {
		if !self.released {
			let mut state = self.adapter.state.lock().unwrap();
			if self.connection {
				state.log.push("connection.drop".into());
			}
			state.log.push("lease.drop".into());
		}
	}
}

struct Batch {
	adapter: Adapter,
	ids: Vec<Uuid>,
}

impl RecoveryBatch for Batch {
	fn ids(&self) -> &[Uuid] {
		&self.ids
	}
}

impl Drop for Batch {
	fn drop(&mut self) {
		self.adapter
			.state
			.lock()
			.unwrap()
			.log
			.push("batch.drop".into());
	}
}

#[async_trait]
impl CoordinatorRepository for Adapter {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn now(&self) -> DateTime<Utc> {
		"2030-01-01T00:00:00Z".parse().unwrap()
	}
	async fn status(&self, id: Uuid) -> Result<Status> {
		self.event(format!("status:{id}"))?;
		let state = self.state.lock().unwrap();
		if state.status.id != id {
			return Err(Error::NotFound("transaction".into()));
		}
		Ok(state.status.clone())
	}
	async fn votes(&self, _id: Uuid) -> Result<Vec<Vote>> {
		self.event("votes")?;
		Ok(self.state.lock().unwrap().votes.clone())
	}
	async fn transition(
		&self,
		_id: Uuid,
		change: CoordinatorTransition,
		detail: &str,
	) -> Result<()> {
		self.event(format!("transition:{}", change.phase()))?;
		let mut state = self.state.lock().unwrap();
		let mut domain = CoordinatorState {
			decision: state.status.decision.as_deref().map(|decision| {
				if decision == "COMMIT" {
					CoordinatorDecision::Commit
				} else {
					CoordinatorDecision::Abort
				}
			}),
			visible: state.status.visible,
			complete: state.status.complete,
		};
		if change.apply(&mut domain)? {
			state.status.decision = domain.decision.map(|decision| match decision {
				CoordinatorDecision::Commit => "COMMIT".into(),
				CoordinatorDecision::Abort => "ABORT".into(),
			});
			state.status.visible = domain.visible;
			state.status.complete = domain.complete;
			state.status.last_error = (!detail.is_empty()
				&& matches!(change, CoordinatorTransition::Decide(_)))
			.then(|| detail.into());
		}
		Ok(())
	}
	async fn acquire_recovery(&self, _id: Uuid) -> Result<Box<dyn RecoveryScope>> {
		self.event("lease.acquire")?;
		if self.state.lock().unwrap().busy {
			return Err(Error::TransactionPending);
		}
		Ok(Box::new(Scope {
			adapter: self.clone(),
			connection: false,
			released: false,
		}))
	}
	async fn recovery_batch(&self, aborted: bool) -> Result<Box<dyn RecoveryBatch>> {
		self.event(format!("batch:{aborted}"))?;
		Ok(Box::new(Batch {
			adapter: self.clone(),
			ids: vec![Uuid::from_u128(99), self.state.lock().unwrap().status.id],
		}))
	}
	async fn issue_authority(&self, _manifest: &Manifest, _node: &str) -> Result<()> {
		self.event("issue")?;
		if self.state.lock().unwrap().issue_failure {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn settle_authority(&self, _id: Uuid, _node: &str, phase: &str) -> Result<()> {
		self.event(format!("settle:{phase}"))
	}
	async fn scoped(&self, _id: Uuid) -> Result<bool> {
		self.event("scoped")?;
		Ok(self.state.lock().unwrap().scoped)
	}
	async fn fault(&self, _id: Uuid, point: &str) -> Result<()> {
		self.event(point)
	}
}

#[async_trait]
impl ParticipantTransport for Adapter {
	async fn decision(&self, _manifest: &Manifest) -> Result<Status> {
		self.event("remote.decision")?;
		Ok(self.state.lock().unwrap().status.clone())
	}
	async fn send(
		&self,
		manifest: &Manifest,
		node: &str,
		operation: ParticipantOperation,
	) -> Result<LocalStatus> {
		self.event(format!("send:{}:{node}", operation.as_str()))?;
		let pause = self.state.lock().unwrap().pause;
		if pause {
			self.started.notify_one();
			self.continue_send.notified().await;
		}
		let state = self.state.lock().unwrap();
		if let Some(failure) = state.transport_failure {
			return Err(failure.error());
		}
		Ok(state.response.clone().unwrap_or_else(|| LocalStatus {
			id: manifest.id,
			coordinator: manifest.coordinator.clone(),
			digest: state.status.digest.clone(),
			manifest: state.status.manifest.clone(),
			phase: operation.expected_phase(&state.status).into(),
			updated_at: state.status.created_at,
		}))
	}
}

#[rstest]
#[tokio::test]
async fn committed_protocol_uses_one_ordered_step_per_advance(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	let coordinator = adapter.coordinator();
	for phase in [
		"RESERVED",
		"PREPARED",
		"PREPARED",
		"APPLIED",
		"APPLIED",
		"COMMITTED",
		"COMMITTED",
	] {
		coordinator.advance(manifest.id).await.unwrap();
		assert_eq!(adapter.state.lock().unwrap().votes[0].phase, phase);
	}
	let state = adapter.state.lock().unwrap();
	assert_eq!(state.status.decision.as_deref(), Some("COMMIT"));
	assert_eq!((state.status.visible, state.status.complete), (true, true));
	assert_eq!(
		state.log.iter().filter(|event| *event == "issue").count(),
		1
	);
	assert_eq!(
		state
			.log
			.iter()
			.filter(|event| *event == "transition:COMMIT")
			.count(),
		1
	);
	let reserve = state
		.log
		.iter()
		.position(|event| event == "send:reserve:aidash://home")
		.unwrap();
	assert_eq!(state.log[reserve - 1], "issue");
	let acknowledgement = state
		.log
		.iter()
		.position(|event| event == "ack:aidash://home:RESERVED")
		.unwrap();
	assert_eq!(
		&state.log[acknowledgement - 2..acknowledgement + 2],
		&[
			"coordinator.vote.before",
			"settle:RESERVED",
			"ack:aidash://home:RESERVED",
			"coordinator.vote.after",
		]
	);
}

#[rstest]
#[case::conflict(Failure::Conflict)]
#[case::invalid(Failure::Invalid)]
#[case::not_found(Failure::NotFound)]
#[case::forbidden(Failure::Forbidden)]
#[case::unauthorized(Failure::Unauthorized)]
#[tokio::test]
async fn deterministic_rejection_aborts_only_an_undecided_transaction(
	manifest: Manifest,
	#[case] failure: Failure,
) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().transport_failure = Some(failure);
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision.as_deref(), Some("ABORT"));
	assert_eq!(
		state.last_error.as_deref(),
		Some(failure.error().to_string().as_str())
	);
	assert!(!adapter.log().contains(&"record_error".into()));
	adapter.state.lock().unwrap().status.decision = Some("COMMIT".into());
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision.as_deref(), Some("COMMIT"));
	assert!(adapter.log().contains(&"record_error".into()));
}

#[rstest]
#[case::scoped(true, Some("ABORT"))]
#[case::unscoped(false, None)]
#[tokio::test]
async fn pending_authority_aborts_only_scoped_admission(
	manifest: Manifest,
	#[case] scoped: bool,
	#[case] decision: Option<&str>,
) {
	let adapter = Adapter::new(&manifest);
	{
		let mut state = adapter.state.lock().unwrap();
		state.transport_failure = Some(Failure::Pending);
		state.scoped = scoped;
	}
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision.as_deref(), decision);
	assert!(adapter.log().contains(&"scoped".into()));
}

#[rstest]
#[tokio::test]
async fn unknown_outcome_is_retained_without_a_decision_or_acknowledgement(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().transport_failure = Some(Failure::Unknown);
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision, None);
	assert_eq!(state.last_error.as_deref(), Some("outcome unknown"));
	assert_eq!(adapter.state.lock().unwrap().votes[0].phase, "PENDING");
	let log = adapter.log();
	assert_eq!(
		&log[log.len() - 2..],
		&["connection.release", "lease.release"]
	);
}

#[rstest]
#[tokio::test]
async fn authority_denial_prevents_participant_io(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().issue_failure = true;
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision.as_deref(), Some("ABORT"));
	assert!(!adapter.log().iter().any(|event| event.starts_with("send:")));
}

#[rstest]
#[case::before_settlement("coordinator.vote.before")]
#[case::during_settlement("settle:RESERVED")]
#[tokio::test]
async fn failed_settlement_cannot_acknowledge_the_vote(manifest: Manifest, #[case] failure: &str) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().fail_event = Some(failure.into());
	assert!(
		matches!(adapter.coordinator().advance(manifest.id).await, Err(Error::External(message)) if message == failure)
	);
	assert_eq!(adapter.state.lock().unwrap().votes[0].phase, "PENDING");
	let log = adapter.log();
	assert_eq!(
		&log[log.len() - 2..],
		&["connection.release", "lease.release"]
	);
}

#[rstest]
#[case::manifest("manifest", "participant acknowledged another manifest")]
#[case::phase("phase", "participant returned an unexpected phase")]
#[tokio::test]
async fn invalid_acknowledgement_releases_lease_without_settlement(
	manifest: Manifest,
	#[case] changed: &str,
	#[case] expected: &str,
) {
	let adapter = Adapter::new(&manifest);
	{
		let mut state = adapter.state.lock().unwrap();
		state.response = Some(LocalStatus {
			id: if changed == "manifest" {
				Uuid::from_u128(99)
			} else {
				manifest.id
			},
			coordinator: manifest.coordinator.clone(),
			digest: state.status.digest.clone(),
			manifest: state.status.manifest.clone(),
			phase: if changed == "phase" {
				"COMMITTED"
			} else {
				"RESERVED"
			}
			.into(),
			updated_at: state.status.created_at,
		});
	}
	assert!(
		matches!(adapter.coordinator().advance(manifest.id).await, Err(Error::Domain(aidash_domain::Error::Conflict(message))) if message == expected)
	);
	assert!(
		!adapter
			.log()
			.iter()
			.any(|event| event.starts_with("settle:"))
	);
	assert_eq!(adapter.state.lock().unwrap().votes[0].phase, "PENDING");
	let log = adapter.log();
	assert_eq!(
		&log[log.len() - 2..],
		&["connection.release", "lease.release"]
	);
}

#[rstest]
#[tokio::test]
async fn operator_abort_ignores_recovery_contention_and_preserves_commit(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().busy = true;
	assert_eq!(
		adapter
			.coordinator()
			.abort(manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	adapter.state.lock().unwrap().status.decision = Some("COMMIT".into());
	assert!(
		matches!(adapter.coordinator().abort(manifest.id).await, Err(Error::Conflict(message)) if message == "commit is irrevocable")
	);
	assert!(!adapter.log().contains(&"lease.acquire".into()));
}

#[rstest]
#[case::expired(false, Some("ABORT"))]
#[case::committed(true, Some("COMMIT"))]
#[tokio::test]
async fn deadline_cannot_replace_a_durable_commit(
	mut manifest: Manifest,
	#[case] committed: bool,
	#[case] decision: Option<&str>,
) {
	manifest.deadline = "2030-01-01T00:00:00Z".parse().unwrap();
	let adapter = Adapter::new(&manifest);
	if committed {
		adapter.state.lock().unwrap().status.decision = Some("COMMIT".into());
	}
	let state = adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(state.decision.as_deref(), decision);
	assert_eq!(
		adapter.log().iter().any(|event| event.starts_with("send:")),
		committed
	);
}

#[rstest]
#[tokio::test]
async fn completed_transaction_does_not_touch_or_contact_participants(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().status.complete = true;
	adapter.coordinator().advance(manifest.id).await.unwrap();
	assert_eq!(
		adapter.log(),
		vec![
			"lease.acquire".into(),
			format!("status:{}", manifest.id),
			"lease.release".into()
		]
	);
}

#[rstest]
#[tokio::test]
async fn lease_release_failure_takes_priority_over_advance_failure(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().fail_event = Some("lease.release".into());
	let error = adapter
		.coordinator()
		.advance(Uuid::from_u128(99))
		.await
		.unwrap_err();
	assert!(matches!(error, Error::External(message) if message == "lease.release"));
}

#[rstest]
#[tokio::test]
async fn cancelling_peer_io_drops_connection_before_recovery_lease(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.state.lock().unwrap().pause = true;
	let coordinator = adapter.coordinator();
	let advance = tokio::spawn(async move { coordinator.advance(manifest.id).await });
	adapter.started.notified().await;
	advance.abort();
	assert!(advance.await.unwrap_err().is_cancelled());
	let log = adapter.log();
	assert_eq!(&log[log.len() - 2..], &["connection.drop", "lease.drop"]);
	assert_eq!(adapter.state.lock().unwrap().status.decision, None);
}

#[rstest]
#[tokio::test]
async fn recovery_keeps_the_batch_alive_and_continues_after_a_failed_id(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.coordinator().recover_kind(false).await.unwrap();
	assert_eq!(adapter.state.lock().unwrap().votes[0].phase, "RESERVED");
	let log = adapter.log();
	assert_eq!(log.first().map(String::as_str), Some("batch:false"));
	assert_eq!(log.last().map(String::as_str), Some("batch.drop"));
	assert_eq!(
		log.iter().filter(|event| *event == "lease.release").count(),
		2
	);
}

#[rstest]
#[case::local(false)]
#[case::remote(true)]
#[tokio::test]
async fn decision_lookup_validates_the_same_manifest_for_local_and_remote_sources(
	mut manifest: Manifest,
	#[case] remote: bool,
) {
	if remote {
		manifest.coordinator = "aidash://peer".into();
	}
	let adapter = Adapter::new(&manifest);
	adapter.coordinator().decision(&manifest).await.unwrap();
	adapter.state.lock().unwrap().status.visible = true;
	assert!(
		matches!(adapter.coordinator().decision(&manifest).await, Err(Error::Conflict(message)) if message == "coordinator decision does not match participant manifest")
	);
	assert_eq!(adapter.log().contains(&"remote.decision".into()), remote);
}

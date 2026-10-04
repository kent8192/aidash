use super::*;
use crate::ports::{
	Credentials,
	registry::CoreToolCatalog,
	transactions::{
		coordination::{CoordinatorRepository, ParticipantTransport, RecoveryBatch, RecoveryScope},
		participation::PendingParticipants,
	},
};
use aidash_domain::{
	capabilities::CoreCapabilities,
	provider::ToolSpec,
	transactions::{
		CoordinatorTransition, Isolation, Mutation, Participant as Node,
		authority::Status,
		coordination::{ParticipantOperation, Vote},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{collections::BTreeMap, sync::Mutex};
use uuid::Uuid;

struct Catalog;
impl Credentials for Catalog {
	fn resolve(&self, name: &str) -> Result<String> {
		Err(Error::NotFound(name.into()))
	}
}
impl CoreToolCatalog for Catalog {
	fn specifications(&self, _config: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}

#[fixture]
fn manifest() -> Manifest {
	Manifest {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T01:00:00Z".parse().unwrap(),
		participants: vec![Node {
			node_id: "aidash://home".into(),
			mutations: vec![Mutation::WorkspaceState {
				workspace_id: Uuid::from_u128(2),
				expected_revision: 0,
				state: json!({}),
			}],
		}],
	}
}

fn row(manifest: &Manifest, phase: &str) -> LocalStatus {
	LocalStatus {
		id: manifest.id,
		coordinator: manifest.coordinator.clone(),
		digest: manifest.digest().unwrap(),
		manifest: json!(manifest),
		phase: phase.into(),
		updated_at: "2030-01-01T00:00:00Z".parse().unwrap(),
	}
}

#[derive(Clone, Default)]
struct Records {
	row: Option<LocalStatus>,
	gate: Option<Uuid>,
	applied: usize,
}

struct State {
	proof: Status,
	persisted: Records,
	log: Vec<String>,
	admitted: bool,
	deny_admission: bool,
	failure: Option<String>,
}

#[derive(Clone)]
struct Adapter(Arc<Mutex<State>>);
impl Adapter {
	fn new(manifest: &Manifest) -> Self {
		Self(Arc::new(Mutex::new(State {
			proof: Status {
				id: manifest.id,
				digest: manifest.digest().unwrap(),
				manifest: json!(manifest),
				decision: None,
				visible: false,
				complete: false,
				last_error: None,
				created_at: "2030-01-01T00:00:00Z".parse().unwrap(),
			},
			persisted: Records::default(),
			log: vec![],
			admitted: false,
			deny_admission: false,
			failure: None,
		})))
	}
	fn participant(&self) -> Participant {
		Participant::new(
			Arc::new(self.clone()),
			Coordinator::new(Arc::new(self.clone()), Arc::new(self.clone())),
			DefinitionValidation::new(Arc::new(Catalog), Arc::new(Catalog)),
		)
	}
	fn event(&self, event: impl Into<String>) -> Result<()> {
		let event = event.into();
		let mut state = self.0.lock().unwrap();
		state.log.push(event.clone());
		if state.failure.as_ref() == Some(&event) {
			return Err(Error::External(event));
		}
		Ok(())
	}
	fn log(&self) -> Vec<String> {
		self.0.lock().unwrap().log.clone()
	}
	fn scope(&self, admitted: bool) -> Box<dyn ParticipantScope> {
		Box::new(Scope {
			adapter: self.clone(),
			working: self.0.lock().unwrap().persisted.clone(),
			admitted,
			finished: false,
		})
	}
	fn existing(&self, manifest: &Manifest, phase: &str) {
		let mut state = self.0.lock().unwrap();
		state.persisted.row = Some(row(manifest, phase));
		state.persisted.gate = Some(manifest.id);
	}
}

struct Scope {
	adapter: Adapter,
	working: Records,
	admitted: bool,
	finished: bool,
}
#[async_trait]
impl ParticipantScope for Scope {
	async fn lock(&mut self, _id: Uuid) -> Result<Option<LocalStatus>> {
		self.adapter.event("lock")?;
		Ok(self.working.row.clone())
	}
	async fn trusted(&mut self, _node: &str) -> Result<bool> {
		self.adapter.event("trusted")?;
		Ok(true)
	}
	async fn gate_exclusive(&mut self) -> Result<Option<Uuid>> {
		self.adapter.event("gate")?;
		Ok(self.working.gate)
	}
	async fn reserve_gate(&mut self, id: Uuid) -> Result<()> {
		self.adapter.event("reserve_gate")?;
		self.working.gate = Some(id);
		Ok(())
	}
	async fn release_gate(&mut self, committed: bool) -> Result<()> {
		self.adapter.event(format!("release_gate:{committed}"))?;
		self.working.gate = None;
		Ok(())
	}
	async fn insert(
		&mut self,
		manifest: &Manifest,
		phase: ParticipantPhase,
	) -> Result<LocalStatus> {
		self.adapter.event(format!("insert:{}", phase.as_str()))?;
		let row = row(manifest, phase.as_str());
		self.working.row = Some(row.clone());
		Ok(row)
	}
	async fn transition(&mut self, _id: Uuid, phase: ParticipantPhase) -> Result<LocalStatus> {
		self.adapter
			.event(format!("transition:{}", phase.as_str()))?;
		let row = self.working.row.as_mut().unwrap();
		row.phase = phase.as_str().into();
		Ok(row.clone())
	}
	async fn reservation_history(&mut self, _id: Uuid) -> Result<()> {
		self.adapter.event("history:RESERVED")
	}
	async fn validate_mutations(&mut self, _manifest: &Manifest) -> Result<()> {
		self.adapter.event("validate_mutations")
	}
	async fn apply_mutations(&mut self, _manifest: &Manifest) -> Result<()> {
		self.adapter.event("apply_mutations")?;
		self.working.applied += 1;
		Ok(())
	}
	async fn finish(mut self: Box<Self>, result: Result<LocalStatus>) -> Result<LocalStatus> {
		match result {
			Ok(row) => {
				self.adapter.event("commit")?;
				self.adapter.0.lock().unwrap().persisted = self.working.clone();
				self.finished = true;
				Ok(row)
			}
			Err(error) => {
				if self.admitted {
					self.adapter.event("authority_failure")?;
				}
				Err(error)
			}
		}
	}
	async fn rollback(mut self: Box<Self>) -> Result<()> {
		self.adapter.event("rollback.probe")?;
		self.finished = true;
		Ok(())
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.finished {
			self.adapter
				.0
				.lock()
				.unwrap()
				.log
				.push("rollback.drop".into());
		}
	}
}

struct Pending {
	adapter: Adapter,
	rows: Vec<LocalStatus>,
}
impl PendingParticipants for Pending {
	fn rows(&self) -> &[LocalStatus] {
		&self.rows
	}
}
impl Drop for Pending {
	fn drop(&mut self) {
		self.adapter
			.0
			.lock()
			.unwrap()
			.log
			.push("pending.drop".into());
	}
}

#[async_trait]
impl ParticipantRepository for Adapter {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	async fn begin(&self) -> Result<Box<dyn ParticipantScope>> {
		self.event("begin")?;
		Ok(self.scope(false))
	}
	async fn admission(
		&self,
		_caller: &str,
		_manifest: &Manifest,
	) -> Result<Option<Box<dyn ParticipantScope>>> {
		self.event("admission")?;
		let (deny, admitted) = {
			let state = self.0.lock().unwrap();
			(state.deny_admission, state.admitted)
		};
		if deny {
			return Err(Error::Forbidden);
		}
		Ok(admitted.then(|| self.scope(true)))
	}
	async fn pending(&self) -> Result<Box<dyn PendingParticipants>> {
		self.event("pending")?;
		let rows = self
			.0
			.lock()
			.unwrap()
			.persisted
			.row
			.iter()
			.cloned()
			.collect();
		Ok(Box::new(Pending {
			adapter: self.clone(),
			rows,
		}))
	}
	async fn fault(&self, _id: Uuid, point: &str) -> Result<()> {
		self.event(point)
	}
	fn wake(&self) {
		self.0.lock().unwrap().log.push("wake".into());
	}
}

fn unexpected<T>() -> Result<T> {
	Err(Error::External(
		"unexpected coordinator write or participant dispatch".into(),
	))
}
#[async_trait]
impl CoordinatorRepository for Adapter {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn now(&self) -> DateTime<Utc> {
		"2030-01-01T00:00:00Z".parse().unwrap()
	}
	async fn status(&self, _id: Uuid) -> Result<Status> {
		self.event("proof")?;
		Ok(self.0.lock().unwrap().proof.clone())
	}
	async fn votes(&self, _id: Uuid) -> Result<Vec<Vote>> {
		unexpected()
	}
	async fn transition(
		&self,
		_id: Uuid,
		_change: CoordinatorTransition,
		_detail: &str,
	) -> Result<()> {
		unexpected()
	}
	async fn acquire_recovery(&self, _id: Uuid) -> Result<Box<dyn RecoveryScope>> {
		unexpected()
	}
	async fn recovery_batch(&self, _aborted: bool) -> Result<Box<dyn RecoveryBatch>> {
		unexpected()
	}
	async fn issue_authority(&self, _manifest: &Manifest, _node: &str) -> Result<()> {
		unexpected()
	}
	async fn settle_authority(&self, _id: Uuid, _node: &str, _phase: &str) -> Result<()> {
		unexpected()
	}
	async fn scoped(&self, _id: Uuid) -> Result<bool> {
		unexpected()
	}
	async fn fault(&self, _id: Uuid, _point: &str) -> Result<()> {
		unexpected()
	}
}
#[async_trait]
impl ParticipantTransport for Adapter {
	async fn decision(&self, _manifest: &Manifest) -> Result<Status> {
		self.event("proof")?;
		Ok(self.0.lock().unwrap().proof.clone())
	}
	async fn send(
		&self,
		_manifest: &Manifest,
		_node: &str,
		_operation: ParticipantOperation,
	) -> Result<LocalStatus> {
		unexpected()
	}
}

#[rstest]
#[tokio::test]
async fn existing_reservation_survives_live_grant_revocation(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "RESERVED");
	adapter.0.lock().unwrap().deny_admission = true;
	let result = adapter
		.participant()
		.reserve(&manifest.coordinator, &manifest)
		.await
		.unwrap();
	assert_eq!(result.phase, "RESERVED");
	assert_eq!(adapter.log(), ["begin", "lock", "commit"]);
}

#[rstest]
#[tokio::test]
async fn new_reservation_requires_live_admission_after_rolling_back_its_probe(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.0.lock().unwrap().deny_admission = true;
	assert!(matches!(
		adapter
			.participant()
			.reserve(&manifest.coordinator, &manifest)
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		adapter.log(),
		["begin", "lock", "rollback.probe", "admission"]
	);
	assert_eq!(adapter.0.lock().unwrap().persisted.row, None);
}

#[rstest]
#[case::standalone(false)]
#[case::admitted(true)]
#[tokio::test]
async fn failed_reservation_never_publishes_a_barrier_or_row(
	manifest: Manifest,
	#[case] admitted: bool,
) {
	let adapter = Adapter::new(&manifest);
	{
		let mut state = adapter.0.lock().unwrap();
		state.admitted = admitted;
		state.failure = Some("participant.reserve.before".into());
	}
	assert!(
		matches!(adapter.participant().reserve(&manifest.coordinator, &manifest).await, Err(Error::External(message)) if message == "participant.reserve.before")
	);
	let state = adapter.0.lock().unwrap();
	assert_eq!(state.persisted.row, None);
	assert_eq!(state.persisted.gate, None);
	assert_eq!(state.log.last().map(String::as_str), Some("rollback.drop"));
}

#[rstest]
#[case::standalone(false)]
#[case::admitted(true)]
#[tokio::test]
async fn admitted_failure_retains_authority_settlement_before_rollback(
	manifest: Manifest,
	#[case] admitted: bool,
) {
	let adapter = Adapter::new(&manifest);
	{
		let mut state = adapter.0.lock().unwrap();
		state.admitted = admitted;
		state.persisted.gate = Some(Uuid::from_u128(99));
	}
	assert!(matches!(
		adapter
			.participant()
			.reserve(&manifest.coordinator, &manifest)
			.await,
		Err(Error::TransactionPending)
	));
	let log = adapter.log();
	assert_eq!(log.contains(&"authority_failure".into()), admitted);
	assert_eq!(log.contains(&"participant.reserve.before".into()), admitted);
	assert_eq!(adapter.0.lock().unwrap().persisted.row, None);
}

#[rstest]
#[case::valid(false)]
#[case::invalid(true)]
#[tokio::test]
async fn prepare_validates_without_applying_mutations(manifest: Manifest, #[case] fails: bool) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "RESERVED");
	if fails {
		adapter.0.lock().unwrap().failure = Some("validate_mutations".into());
	}
	let result = adapter
		.participant()
		.prepare(&manifest.coordinator, &manifest)
		.await;
	let state = adapter.0.lock().unwrap();
	assert_eq!(state.persisted.applied, 0);
	assert_eq!(
		state.persisted.row.as_ref().unwrap().phase,
		if fails { "RESERVED" } else { "PREPARED" }
	);
	if fails {
		assert!(matches!(result, Err(Error::External(message)) if message == "validate_mutations"));
	} else {
		assert_eq!(result.unwrap().phase, "PREPARED");
	}
}

#[rstest]
#[tokio::test]
async fn apply_and_visibility_release_are_separate_idempotent_commits(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "PREPARED");
	adapter.0.lock().unwrap().proof.decision = Some("COMMIT".into());
	let participant = adapter.participant();
	for _ in 0..2 {
		assert_eq!(
			participant
				.finish(&manifest.coordinator, &manifest)
				.await
				.unwrap()
				.phase,
			"APPLIED"
		);
	}
	{
		let mut state = adapter.0.lock().unwrap();
		assert_eq!(state.persisted.applied, 1);
		assert_eq!(state.persisted.gate, Some(manifest.id));
		state.proof.visible = true;
	}
	for _ in 0..2 {
		assert_eq!(
			participant
				.finish(&manifest.coordinator, &manifest)
				.await
				.unwrap()
				.phase,
			"COMMITTED"
		);
	}
	let state = adapter.0.lock().unwrap();
	assert_eq!(state.persisted.applied, 1);
	assert_eq!(state.persisted.gate, None);
	assert_eq!(
		state
			.log
			.iter()
			.filter(|event| *event == "release_gate:true")
			.count(),
		1
	);
}

#[rstest]
#[tokio::test]
async fn abort_tombstone_prevents_a_delayed_reservation(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.0.lock().unwrap().proof.decision = Some("ABORT".into());
	let participant = adapter.participant();
	assert_eq!(
		participant
			.finish(&manifest.coordinator, &manifest)
			.await
			.unwrap()
			.phase,
		"ABORTED"
	);
	adapter.0.lock().unwrap().deny_admission = true;
	assert_eq!(
		participant
			.reserve(&manifest.coordinator, &manifest)
			.await
			.unwrap()
			.phase,
		"ABORTED"
	);
	assert_eq!(adapter.0.lock().unwrap().persisted.gate, None);
	assert_eq!(adapter.log().contains(&"admission".into()), false);
}

#[rstest]
#[case::applied("APPLIED")]
#[case::committed("COMMITTED")]
#[tokio::test]
async fn durable_commit_cannot_be_aborted(manifest: Manifest, #[case] phase: &str) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, phase);
	adapter.0.lock().unwrap().proof.decision = Some("ABORT".into());
	assert!(
		matches!(adapter.participant().finish(&manifest.coordinator, &manifest).await, Err(Error::Conflict(message)) if message == "commit cannot be aborted")
	);
	assert_eq!(
		adapter
			.0
			.lock()
			.unwrap()
			.persisted
			.row
			.as_ref()
			.unwrap()
			.phase,
		phase
	);
	assert_eq!(adapter.log().contains(&"release_gate:false".into()), false);
}

#[rstest]
#[case::before_commit("participant.apply.before", 0, "PREPARED")]
#[case::after_commit("participant.apply.after", 1, "APPLIED")]
#[tokio::test]
async fn apply_faults_preserve_the_durable_commit_boundary(
	manifest: Manifest,
	#[case] point: &str,
	#[case] applied: usize,
	#[case] phase: &str,
) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "PREPARED");
	{
		let mut state = adapter.0.lock().unwrap();
		state.proof.decision = Some("COMMIT".into());
		state.failure = Some(point.into());
	}
	assert!(
		matches!(adapter.participant().finish(&manifest.coordinator, &manifest).await, Err(Error::External(message)) if message == point)
	);
	let state = adapter.0.lock().unwrap();
	assert_eq!(state.persisted.applied, applied);
	assert_eq!(state.persisted.row.as_ref().unwrap().phase, phase);
	assert_eq!(state.persisted.gate, Some(manifest.id));
	assert_eq!(state.log.contains(&"wake".into()), false);
}

#[rstest]
#[case::pending(false)]
#[case::unavailable(true)]
#[tokio::test]
async fn unavailable_or_missing_decision_cannot_release_the_barrier(
	manifest: Manifest,
	#[case] unavailable: bool,
) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "PREPARED");
	if unavailable {
		adapter.0.lock().unwrap().failure = Some("proof".into());
	}
	let result = adapter
		.participant()
		.finish(&manifest.coordinator, &manifest)
		.await;
	if unavailable {
		assert!(matches!(result, Err(Error::External(message)) if message == "proof"));
	} else {
		assert!(matches!(result, Err(Error::TransactionPending)));
	}
	assert_eq!(adapter.log(), ["proof"]);
	assert_eq!(adapter.0.lock().unwrap().persisted.gate, Some(manifest.id));
}

#[rstest]
#[tokio::test]
async fn sender_must_be_the_named_coordinator_before_any_persistence(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	assert!(matches!(
		adapter
			.participant()
			.reserve("aidash://other", &manifest)
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(adapter.log(), Vec::<String>::new());
}

#[rstest]
#[tokio::test]
async fn recovery_rechecks_the_durable_decision_before_finalization(manifest: Manifest) {
	let adapter = Adapter::new(&manifest);
	adapter.existing(&manifest, "PREPARED");
	adapter.0.lock().unwrap().proof.decision = Some("COMMIT".into());
	assert_eq!(adapter.participant().recover_once().await.unwrap(), 1);
	let log = adapter.log();
	assert_eq!(log.iter().filter(|event| *event == "proof").count(), 2);
	assert_eq!(log.last().map(String::as_str), Some("pending.drop"));
	assert_eq!(
		adapter
			.0
			.lock()
			.unwrap()
			.persisted
			.row
			.as_ref()
			.unwrap()
			.phase,
		"APPLIED"
	);
}

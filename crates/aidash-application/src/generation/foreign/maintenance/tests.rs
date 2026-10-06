use super::*;
use crate::{
	generation::test_support,
	ports::generation::{
		foreign::maintenance::ForeignGenerationVisibility, provisioning::GenerationTerminalSession,
	},
};
use aidash_domain::RunPhase;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub(crate) struct Repository(pub(crate) Arc<Mutex<World>>);
pub(crate) struct World {
	pub(crate) events: Vec<&'static str>,
	pub(crate) failure: Option<&'static str>,
	pub(crate) warnings: Vec<(Uuid, String, bool)>,
	pause: Option<&'static str>,
	claimed: bool,
	acknowledged: bool,
	delivered: bool,
	visibility: usize,
	pending: Vec<(Uuid, Value)>,
	jobs: Vec<Request>,
	job: Option<Request>,
	phase: RunPhase,
	reloaded: Option<Request>,
	transitions: Vec<(Uuid, String)>,
	commits: usize,
	notifications: usize,
}
struct Visibility {
	world: Arc<Mutex<World>>,
	active: bool,
}
struct Terminal(Arc<Mutex<World>>);
impl Drop for Visibility {
	fn drop(&mut self) {
		if self.active {
			self.world.lock().unwrap().visibility -= 1;
		}
	}
}
async fn point(world: &Arc<Mutex<World>>, name: &'static str) -> Result<()> {
	let (failure, pause) = {
		let mut w = world.lock().unwrap();
		w.events.push(name);
		(w.failure, w.pause)
	};
	if pause == Some(name) {
		std::future::pending::<()>().await;
	}
	if failure == Some(name) {
		return Err(Error::External(format!("{name} failed")));
	}
	Ok(())
}
#[fixture]
pub(crate) fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(World {
		events: vec![],
		failure: None,
		warnings: vec![],
		pause: None,
		claimed: false,
		acknowledged: true,
		delivered: false,
		visibility: 0,
		pending: vec![],
		jobs: vec![],
		job: None,
		phase: serde_json::from_value(json!("COMPLETED")).unwrap(),
		reloaded: None,
		transitions: vec![],
		commits: 0,
		notifications: 0,
	})))
}
fn intent() -> Value {
	json!({"id":Uuid::from_u128(4),"home_node":"aidash://home","source_tenant":"tenant","source_subject":"root","task":{"id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"title":"Task","description":"Work","status":"OPEN","requirements":{},"owner":null,"created_by":"root","dependencies":[],"parent_id":null,"revision":7,"created_at":"1970-01-01T00:16:40Z"},"target_node":"aidash://executor","policy_id":"policy","policy_revision":7,"lineage":[],"reason":"Generate","ttl_seconds":1000,"expires_at":"1970-01-01T00:33:20Z"})
}
#[async_trait]
impl ForeignGenerationVisibility for Visibility {
	async fn suspend(&mut self) -> Result<()> {
		point(&self.world, "suspend").await?;
		assert!(self.active);
		self.world.lock().unwrap().visibility -= 1;
		self.active = false;
		Ok(())
	}
}
#[async_trait]
impl ForeignGenerationMaintenance for Repository {
	fn now(&self) -> DateTime<Utc> {
		DateTime::from_timestamp(1000, 0).unwrap()
	}
	async fn begin_visibility(&self) -> Result<Box<dyn ForeignGenerationVisibility>> {
		point(&self.0, "visibility").await?;
		self.0.lock().unwrap().visibility += 1;
		Ok(Box::new(Visibility {
			world: self.0.clone(),
			active: true,
		}))
	}
	async fn reserve_retry(&self, id: Uuid) -> Result<u64> {
		assert_eq!(id, Uuid::from_u128(4));
		point(&self.0, "reserve").await?;
		let mut w = self.0.lock().unwrap();
		let count = u64::from(!w.claimed);
		w.claimed = true;
		Ok(count)
	}
	async fn send_cancel(&self, target: &str, id: Uuid) -> Result<bool> {
		assert_eq!((target, id), ("aidash://executor", Uuid::from_u128(4)));
		assert_eq!(
			self.0.lock().unwrap().visibility,
			0,
			"network I/O must not hold local visibility"
		);
		point(&self.0, "send").await?;
		Ok(self.0.lock().unwrap().acknowledged)
	}
	async fn mark_delivered(&self, id: Uuid) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(4));
		point(&self.0, "delivered").await?;
		self.0.lock().unwrap().delivered = true;
		Ok(())
	}
	async fn pending(&self) -> Result<Vec<(Uuid, Value)>> {
		assert_eq!(self.0.lock().unwrap().visibility, 1);
		point(&self.0, "pending").await?;
		Ok(self.0.lock().unwrap().pending.clone())
	}
	async fn jobs(&self) -> Result<Vec<Request>> {
		assert_eq!(self.0.lock().unwrap().visibility, 1);
		point(&self.0, "jobs").await?;
		Ok(self.0.lock().unwrap().jobs.clone())
	}
	async fn cancel_job(&self, source: &str, id: Uuid) -> Result<Option<Request>> {
		assert_eq!((source, id), ("aidash://home", Uuid::from_u128(4)));
		point(&self.0, "cancel_job").await?;
		Ok(self.0.lock().unwrap().job.clone())
	}
	async fn run_phase(&self, id: Uuid) -> Result<RunPhase> {
		assert_eq!(id, Uuid::from_u128(5));
		point(&self.0, "phase").await?;
		Ok(self.0.lock().unwrap().phase)
	}
	async fn begin_terminal(&self, _: &Request) -> Result<Box<dyn GenerationTerminalSession>> {
		point(&self.0, "writer").await?;
		Ok(Box::new(Terminal(self.0.clone())))
	}
	fn warn_cancel(&self, id: Uuid, error: &Error, retry: bool) {
		self.0
			.lock()
			.unwrap()
			.warnings
			.push((id, error.to_string(), retry));
	}
	fn notify(&self) {
		self.0.lock().unwrap().notifications += 1;
	}
}
#[async_trait]
impl GenerationTerminalSession for Terminal {
	async fn load(&mut self, tenant: &str, id: Uuid) -> Result<Request> {
		point(&self.0, "load").await?;
		let w = self.0.lock().unwrap();
		let job = w
			.reloaded
			.clone()
			.unwrap_or_else(|| test_support::request(9));
		assert_eq!((tenant, id), (job.tenant.as_str(), job.id));
		Ok(job)
	}
	async fn run_phase(&mut self, _: &Request) -> Result<Option<String>> {
		panic!("foreign termination must not add the local provisioning phase lookup")
	}
	async fn transition(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> Result<()> {
		assert_eq!(
			(actor, reason),
			(
				"generation-service",
				"foreign generation lifecycle reconciled"
			)
		);
		point(&self.0, "transition").await?;
		self.0
			.lock()
			.unwrap()
			.transitions
			.push((job.id, status.into()));
		Ok(())
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		result?;
		point(&self.0, "commit").await?;
		self.0.lock().unwrap().commits += 1;
		Ok(())
	}
}
#[rstest]
#[tokio::test]
async fn cancellation_claims_before_rpc_and_records_only_acknowledged_delivery(
	repository: Repository,
) {
	deliver_cancel(&repository, Uuid::from_u128(4), "aidash://executor")
		.await
		.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(
		w.events,
		vec!["visibility", "reserve", "suspend", "send", "delivered"]
	);
	assert!(w.claimed);
	assert!(w.delivered);
	assert_eq!(w.visibility, 0);
}
#[rstest]
#[tokio::test]
async fn previously_claimed_retry_slot_skips_duplicate_rpc(repository: Repository) {
	repository.0.lock().unwrap().claimed = true;
	deliver_cancel(&repository, Uuid::from_u128(4), "aidash://executor")
		.await
		.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(w.events, vec!["visibility", "reserve", "suspend"]);
	assert!(!w.delivered);
	assert_eq!(w.visibility, 0);
}
#[rstest]
#[tokio::test]
async fn rejected_acknowledgement_keeps_the_durable_retry_slot(repository: Repository) {
	repository.0.lock().unwrap().acknowledged = false;
	assert!(
		matches!(deliver_cancel(&repository,Uuid::from_u128(4),"aidash://executor").await,Err(Error::Conflict(message)) if message=="remote generation cancellation was not acknowledged")
	);
	let w = repository.0.lock().unwrap();
	assert!(w.claimed);
	assert!(!w.delivered);
}
#[rstest]
#[case("visibility", false)]
#[case("reserve", false)]
#[case("suspend", true)]
#[case("send", true)]
#[case("delivered", true)]
#[tokio::test]
async fn cancellation_failure_preserves_exact_error_and_durable_claim(
	repository: Repository,
	#[case] operation: &'static str,
	#[case] claimed: bool,
) {
	repository.0.lock().unwrap().failure = Some(operation);
	assert!(
		matches!(deliver_cancel(&repository,Uuid::from_u128(4),"aidash://executor").await,Err(Error::External(message)) if message==format!("{operation} failed"))
	);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.claimed, claimed);
	assert!(!w.delivered);
	assert_eq!(w.visibility, 0);
}
#[rstest]
#[tokio::test]
async fn cancelled_rpc_retains_its_claim_and_cannot_hot_loop(repository: Repository) {
	repository.0.lock().unwrap().pause = Some("send");
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(20),
			deliver_cancel(&repository, Uuid::from_u128(4), "aidash://executor")
		)
		.await
		.is_err()
	);
	deliver_cancel(&repository, Uuid::from_u128(4), "aidash://executor")
		.await
		.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(w.events.iter().filter(|name| **name == "send").count(), 1);
	assert!(!w.delivered);
	assert_eq!(w.visibility, 0);
}
#[rstest]
#[tokio::test]
async fn reconciliation_continues_failed_cancel_then_reacquires_visibility(repository: Repository) {
	{
		let mut w = repository.0.lock().unwrap();
		w.pending = vec![(Uuid::from_u128(4), intent())];
		w.failure = Some("send");
		w.jobs = vec![test_support::request(9)];
	}
	reconcile(&repository).await.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(
		w.warnings,
		vec![(Uuid::from_u128(4), "send failed".into(), true)]
	);
	assert_eq!(w.transitions, vec![(Uuid::from_u128(9), "EXPIRED".into())]);
	assert_eq!(w.notifications, 1);
	assert_eq!(w.visibility, 0);
	assert!(
		w.events.iter().position(|name| *name == "send").unwrap()
			< w.events
				.iter()
				.rposition(|name| *name == "visibility")
				.unwrap()
	);
}
#[rstest]
#[case("COMPLETED", "COMPLETED")]
#[case("CANCELLED", "STOPPED")]
#[case("FAILED", "FAILED")]
#[case("THINKING", "FAILED")]
#[tokio::test]
async fn reconciliation_preserves_terminal_phase_mapping(
	repository: Repository,
	#[case] phase: &str,
	#[case] status: &str,
) {
	{
		let mut w = repository.0.lock().unwrap();
		let mut job = test_support::request(9);
		job.expires_at = DateTime::from_timestamp(2000, 0).unwrap();
		job.admission_id = Some(Uuid::from_u128(5));
		w.jobs = vec![job];
		w.phase = serde_json::from_value(json!(phase)).unwrap();
	}
	reconcile(&repository).await.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(w.transitions, vec![(Uuid::from_u128(9), status.into())]);
	assert_eq!(w.commits, 1);
	assert_eq!(w.notifications, 1);
}
#[rstest]
#[tokio::test]
async fn expiry_does_not_decode_the_admission_run(repository: Repository) {
	{
		let mut w = repository.0.lock().unwrap();
		let mut job = test_support::request(9);
		job.admission_id = Some(Uuid::from_u128(5));
		w.jobs = vec![job];
		w.failure = Some("phase");
	}
	reconcile(&repository).await.unwrap();
	let w = repository.0.lock().unwrap();
	assert_eq!(w.transitions, vec![(Uuid::from_u128(9), "EXPIRED".into())]);
	assert!(!w.events.contains(&"phase"));
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn peer_cancellation_is_idempotent_even_after_terminal_transition(
	repository: Repository,
	#[case] exists: bool,
) {
	if exists {
		let mut w = repository.0.lock().unwrap();
		w.job = Some(test_support::request(9));
		let mut current = test_support::request(9);
		current.status = "COMPLETED".into();
		w.reloaded = Some(current);
	}
	assert!(
		cancel_at(&repository, "aidash://home", Uuid::from_u128(4))
			.await
			.unwrap()
	);
	let w = repository.0.lock().unwrap();
	assert!(w.transitions.is_empty());
	assert_eq!(w.notifications, usize::from(exists));
}
#[rstest]
#[case("writer")]
#[case("load")]
#[case("transition")]
#[case("commit")]
#[tokio::test]
async fn terminal_failure_does_not_commit_or_notify(
	repository: Repository,
	#[case] operation: &'static str,
) {
	repository.0.lock().unwrap().failure = Some(operation);
	assert!(
		matches!(terminate(&repository,&test_support::request(9),"STOPPED").await,Err(Error::External(message)) if message==format!("{operation} failed"))
	);
	let w = repository.0.lock().unwrap();
	assert_eq!(w.commits, 0);
	assert_eq!(w.notifications, 0);
}
#[rstest]
#[tokio::test]
async fn malformed_saved_intent_aborts_the_scan_without_network_or_local_transitions(
	repository: Repository,
) {
	repository.0.lock().unwrap().pending = vec![(Uuid::from_u128(4), json!({"invalid":true}))];
	assert!(matches!(reconcile(&repository).await, Err(Error::Json(_))));
	let w = repository.0.lock().unwrap();
	assert_eq!(w.visibility, 0);
	assert!(!w.events.contains(&"send"));
	assert!(!w.events.contains(&"jobs"));
}

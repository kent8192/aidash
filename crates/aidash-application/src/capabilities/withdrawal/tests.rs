use super::*;
use crate::{Error, ports::capabilities::withdrawal::OperationWithdrawalScope};
use aidash_domain::capabilities::operations::withdrawal::{Change, Snapshot};
use async_trait::async_trait;
use futures_util::FutureExt;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
struct State {
	snapshot: Snapshot,
	calls: Vec<&'static str>,
	alive: usize,
	stopped: bool,
	failure: Option<&'static str>,
	pending: bool,
	committed: Option<Change>,
	/// What the Runner answers, when it says more than `stopped`.
	reply: Option<Value>,
	refusals: Vec<Option<Value>>,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);
struct Scope(Arc<Mutex<State>>);
impl Drop for Scope {
	fn drop(&mut self) {
		let mut state = self.0.lock().unwrap();
		state.alive -= 1;
		state.calls.push("drop");
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		snapshot: Snapshot {
			operation_id: Uuid::from_u128(1),
			digest: "immutable-intent".into(),
			runner_instance: Some("original-runner".into()),
			kind: "shell".into(),
			state: "running".into(),
			result: json!({}),
			epoch: 8,
			generation: 3,
			area_epoch: 8,
			area_generation: 3,
			area_state: "running".into(),
		},
		calls: vec![],
		alive: 0,
		stopped: true,
		failure: None,
		pending: false,
		committed: None,
		reply: None,
		refusals: vec![],
	})))
}
#[async_trait]
impl OperationWithdrawalRepository for Repository {
	async fn lock(&self, area: Uuid, operation: Uuid) -> Result<Box<dyn OperationWithdrawalScope>> {
		assert_eq!(area, Uuid::from_u128(2));
		assert_eq!(operation, Uuid::from_u128(1));
		{
			let mut state = self.0.lock().unwrap();
			state.calls.push("lock");
			if state.failure == Some("lock") {
				return Err(Error::External("lock fault".into()));
			}
			state.alive += 1;
		}
		Ok(Box::new(Scope(self.0.clone())))
	}
	async fn cancel_bound(
		&self,
		operation: Uuid,
		digest: &str,
		instance: &str,
		refusal: Option<Value>,
	) -> Result<WriterStop> {
		assert_eq!(digest, "immutable-intent");
		assert_eq!(instance, "original-runner");
		Ok(WriterStop::from_reply(
			&self.cancel(operation, refusal).await?,
		))
	}
	async fn cancel(&self, operation: Uuid, refusal: Option<Value>) -> Result<Value> {
		assert_eq!(operation, Uuid::from_u128(1));
		let pending = {
			let mut state = self.0.lock().unwrap();
			assert_eq!(state.alive, 1);
			state.calls.push("cancel");
			state.refusals.push(refusal);
			if state.failure == Some("cancel") {
				return Err(Error::External("cancel fault".into()));
			}
			state.pending
		};
		if pending {
			futures_util::future::pending::<()>().await;
		}
		let state = self.0.lock().unwrap();
		Ok(state
			.reply
			.clone()
			.unwrap_or_else(|| json!({"termination_confirmed":state.stopped})))
	}
}
#[async_trait]
impl OperationWithdrawalScope for Scope {
	fn snapshot(&self) -> Snapshot {
		self.0.lock().unwrap().snapshot.clone()
	}
	async fn commit(self: Box<Self>, change: Change) -> Result<()> {
		{
			let mut state = self.0.lock().unwrap();
			state.calls.push("commit");
			if state.failure == Some("commit") {
				return Err(Error::External("commit fault".into()));
			}
			state.committed = Some(change);
		}
		Ok(())
	}
}
#[rstest]
#[case::prepared("prepared", true, false)]
#[case::stopped("running", true, true)]
#[case::pending("submitted", false, true)]
#[tokio::test]
async fn cancellation_keeps_locks_until_atomic_commit_and_skips_the_runner_for_undispatched_work(
	repository: Repository,
	#[case] status: &str,
	#[case] stopped: bool,
	#[case] remote: bool,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.snapshot.state = status.into();
		state.stopped = stopped;
	}
	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.alive, 0);
	assert_eq!(
		state.calls,
		if remote {
			vec!["lock", "cancel", "commit", "drop"]
		} else {
			vec!["lock", "commit", "drop"]
		}
	);
	let change = state.committed.as_ref().unwrap();
	assert_eq!(
		change.state,
		if stopped { "withdrawn" } else { "cancelling" }
	);
	assert_eq!(change.result["effects_may_have_occurred"], remote);
}
#[rstest]
#[case::terminal("completed")]
#[case::uncertain("uncertain")]
#[tokio::test]
async fn reloaded_terminal_state_drops_the_transaction_without_cancel_or_commit(
	repository: Repository,
	#[case] status: &str,
) {
	repository.0.lock().unwrap().snapshot.state = status.into();
	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock", "drop"]);
	assert_eq!(state.alive, 0);
	assert!(state.committed.is_none());
}
#[rstest]
#[case::lock("lock")]
#[case::cancel("cancel")]
#[case::commit("commit")]
#[tokio::test]
async fn a_failed_boundary_releases_its_scope_and_preserves_the_original_error(
	repository: Repository,
	#[case] failure: &'static str,
) {
	repository.0.lock().unwrap().failure = Some(failure);
	let result = withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1)).await;
	assert!(matches!(result,Err(Error::External(message)) if message==format!("{failure} fault")));
	let state = repository.0.lock().unwrap();
	assert_eq!(state.alive, 0);
	assert!(state.committed.is_none());
}
#[rstest]
fn cancellation_during_runner_io_drops_the_owning_scope_without_committing(repository: Repository) {
	repository.0.lock().unwrap().pending = true;
	assert!(
		withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
			.now_or_never()
			.is_none()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock", "cancel", "drop"]);
	assert_eq!(state.alive, 0);
	assert!(state.committed.is_none());
}

#[rstest]
#[case::completed("completed", true)]
#[case::uncertain("uncertain", true)]
#[case::still_running("uncertain", false)]
#[tokio::test]
async fn captured_terminal_writer_preserves_saved_outcome(
	repository: Repository,
	#[case] status: &str,
	#[case] stopped: bool,
) {
	let original = json!({"stdout":"saved output", "error":"saved error", "runner_acknowledged":true, "effects_may_have_occurred":true, "termination_confirmed":false});
	{
		let mut state = repository.0.lock().unwrap();
		state.snapshot.state = status.into();
		state.snapshot.result = original.clone();
		state.stopped = stopped;
	}
	stop_captured_writer(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	let change = state.committed.as_ref().unwrap();
	assert_eq!(change.state, status);
	assert!(change.area_state.is_none());
	let mut expected = original;
	expected["termination_confirmed"] = json!(stopped);
	assert_eq!(change.result, expected);
	assert_eq!(state.calls, vec!["lock", "cancel", "commit", "drop"]);
}

#[rstest]
#[tokio::test]
async fn a_saved_terminal_stop_schedules_missing_output_ack_without_repeating_cancel(
	repository: Repository,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.snapshot.state = "uncertain".into();
		state.snapshot.result = json!({"termination_confirmed":true,"stdout":"retained"});
	}
	stop_captured_writer(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock", "commit", "drop"]);
	let change = state.committed.as_ref().unwrap();
	assert_eq!(change.state, "uncertain");
	assert_eq!(
		change.result,
		json!({"termination_confirmed":true,"runner_acknowledged":false,"stdout":"retained"})
	);
}

/// A cancel of an operation that may not have reached the Runner asks it to
/// refuse a later send, then ends on what the Runner proved: a refused or
/// unstarted command changed nothing, and an `absent` proves nothing.
#[rstest]
#[case::refused(json!({"status":"cancelled","termination_confirmed":true,"executed":false}), "withdrawn", false, Some("active"))]
#[case::delivered_and_stopping(json!({"status":"accepted","termination_confirmed":false,"executed":false}), "cancelling", true, Some("uncertain"))]
#[case::lost_journal(json!({"status":"absent"}), "cancelling", true, Some("uncertain"))]
#[tokio::test]
async fn a_withdrawn_pending_operation_ends_on_what_the_runner_proved(
	repository: Repository,
	#[case] reply: Value,
	#[case] status: &str,
	#[case] effects: bool,
	#[case] area: Option<&str>,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.snapshot.state = "submitted".into();
		state.snapshot.result = json!({"dispatch_pending":true});
		state.reply = Some(reply);
	}
	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.refusals,
		vec![Some(
			json!({"digest":"immutable-intent","instance":"original-runner"})
		)]
	);
	let change = state.committed.as_ref().unwrap();
	assert_eq!(change.state, status);
	assert_eq!(change.result["effects_may_have_occurred"], effects);
	assert_eq!(change.area_state, area);
}

#[rstest]
#[case::refused(json!({"status":"cancelled","termination_confirmed":true,"executed":false}), "withdrawn", false, Some("active"))]
#[case::lost_journal(json!({"status":"absent"}), "cancelling", true, Some("uncertain"))]
#[tokio::test]
async fn a_captured_pending_operation_ends_on_what_the_runner_proved(
	repository: Repository,
	#[case] reply: Value,
	#[case] status: &str,
	#[case] effects: bool,
	#[case] area: Option<&str>,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.snapshot.state = "cancelling".into();
		state.snapshot.result = json!({"dispatch_pending":true});
		state.reply = Some(reply);
	}
	stop_captured_writer(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.refusals,
		vec![Some(
			json!({"digest":"immutable-intent","instance":"original-runner"})
		)]
	);
	let change = state.committed.as_ref().unwrap();
	assert_eq!(change.state, status);
	assert_eq!(change.result["effects_may_have_occurred"], effects);
	assert_eq!(change.area_state, area);
}

#[rstest]
#[tokio::test]
async fn a_withdrawal_after_the_runner_answered_asks_for_no_refusal(repository: Repository) {
	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();
	assert_eq!(repository.0.lock().unwrap().refusals, vec![None]);
}

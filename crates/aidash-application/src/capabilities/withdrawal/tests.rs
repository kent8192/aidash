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
	/// Every commit, in order. The first of a dispatched operation's is the marker.
	committed: Vec<Change>,
	/// What the Runner answers, when it says more than `stopped`.
	reply: Option<Value>,
	refusals: Vec<Option<Value>>,
	/// How many scopes were open each time the Runner was asked.
	open_when_asked: Vec<usize>,
	/// The state another pass of the sweep leaves the row in while the Runner is asked.
	settled_by_the_sweep: Option<&'static str>,
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
		committed: vec![],
		reply: None,
		refusals: vec![],
		open_when_asked: vec![],
		settled_by_the_sweep: None,
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
			let open = state.alive;
			state.open_when_asked.push(open);
			state.calls.push("cancel");
			state.refusals.push(refusal);
			if state.failure == Some("cancel") {
				return Err(Error::External("cancel fault".into()));
			}
			if let Some(settled) = state.settled_by_the_sweep {
				state.snapshot.state = settled.into();
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
			state.snapshot.state = change.state.clone();
			state.snapshot.result = change.result.clone();
			if let Some(area) = change.area_state {
				state.snapshot.area_state = area.into();
			}
			state.committed.push(change);
		}
		Ok(())
	}
}
#[rstest]
#[case::prepared("prepared", true, false)]
#[case::stopped("running", true, true)]
#[case::pending("submitted", false, true)]
#[tokio::test]
async fn a_dispatched_operation_is_marked_before_the_runner_is_asked_and_the_stop_ends_it(
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
		match (remote, stopped) {
			// Never dispatched: no Runner, one commit.
			(false, _) => vec!["lock", "commit", "drop"],
			// The marker, the Runner, then the outcome under fresh locks.
			(true, true) => vec!["lock", "commit", "drop", "cancel", "lock", "commit", "drop"],
			// An unconfirmed stop leaves the marker as it is.
			(true, false) => vec!["lock", "commit", "drop", "cancel"],
		}
	);
	assert_eq!(state.open_when_asked, if remote { vec![0] } else { vec![] });
	let change = state.committed.last().unwrap();
	assert_eq!(
		change.state,
		if stopped { "withdrawn" } else { "cancelling" }
	);
	assert_eq!(change.result["effects_may_have_occurred"], remote);
	assert_eq!(change.result["error"], "AUTHORITY_WITHDRAWN");
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
	assert!(state.committed.is_empty());
}
#[rstest]
#[case::lock("lock")]
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
	assert!(state.committed.is_empty());
	assert!(
		state.open_when_asked.is_empty(),
		"the Runner is not asked before the marker is committed"
	);
}
/// Egress refuses a stream from the operation row, so the row must say the
/// authority is gone even when the Runner cannot be asked to stop the command.
#[rstest]
#[tokio::test]
async fn an_unreachable_runner_leaves_the_operation_marked_withdrawn(repository: Repository) {
	repository.0.lock().unwrap().failure = Some("cancel");
	let result = withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1)).await;
	assert!(matches!(result, Err(Error::External(message)) if message == "cancel fault"));
	let state = repository.0.lock().unwrap();
	assert_eq!(state.alive, 0);
	assert_eq!(state.calls, vec!["lock", "commit", "drop", "cancel"]);
	let [marker] = state.committed.as_slice() else {
		panic!("one commit, the marker");
	};
	assert_eq!(marker.state, "cancelling");
	assert_eq!(marker.result["error"], "AUTHORITY_WITHDRAWN");
	assert_eq!(marker.result["termination_confirmed"], false);
	assert_eq!(marker.result["effects_may_have_occurred"], true);
	assert_eq!(marker.area_state, Some("uncertain"));
}
#[rstest]
#[tokio::test]
async fn a_retry_asks_the_runner_again_without_marking_the_operation_twice(repository: Repository) {
	repository.0.lock().unwrap().failure = Some("cancel");
	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap_err();
	{
		let mut state = repository.0.lock().unwrap();
		state.failure = None;
		state.calls.clear();
	}

	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();

	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		vec!["lock", "drop", "cancel", "lock", "commit", "drop"]
	);
	let outcome = state.committed.last().unwrap();
	assert_eq!(outcome.state, "withdrawn");
	assert_eq!(outcome.area_state, Some("active"));
	assert_eq!(state.committed.len(), 2, "the marker and the outcome");
}
#[rstest]
#[tokio::test]
async fn an_operation_the_sweep_ended_meanwhile_keeps_its_outcome(repository: Repository) {
	repository.0.lock().unwrap().settled_by_the_sweep = Some("completed");

	withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
		.await
		.unwrap();

	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		vec!["lock", "commit", "drop", "cancel", "lock", "drop"]
	);
	assert_eq!(state.committed.len(), 1, "only the marker");
	assert_eq!(state.snapshot.state, "completed");
}
#[rstest]
fn cancellation_during_runner_io_leaves_the_marker_and_nothing_else(repository: Repository) {
	repository.0.lock().unwrap().pending = true;
	assert!(
		withdraw(&repository, Uuid::from_u128(2), Uuid::from_u128(1))
			.now_or_never()
			.is_none()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["lock", "commit", "drop", "cancel"]);
	assert_eq!(state.alive, 0);
	let [marker] = state.committed.as_slice() else {
		panic!("one commit, the marker");
	};
	assert_eq!(marker.state, "cancelling");
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
	let change = state.committed.last().unwrap();
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
	let change = state.committed.last().unwrap();
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
	let change = state.committed.last().unwrap();
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
	let change = state.committed.last().unwrap();
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

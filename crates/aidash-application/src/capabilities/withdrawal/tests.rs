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
			state: "running".into(),
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
	async fn cancel(&self, operation: Uuid) -> Result<Value> {
		assert_eq!(operation, Uuid::from_u128(1));
		let pending = {
			let mut state = self.0.lock().unwrap();
			assert_eq!(state.alive, 1);
			state.calls.push("cancel");
			if state.failure == Some("cancel") {
				return Err(Error::External("cancel fault".into()));
			}
			state.pending
		};
		if pending {
			futures_util::future::pending::<()>().await;
		}
		Ok(json!({"termination_confirmed":self.0.lock().unwrap().stopped}))
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

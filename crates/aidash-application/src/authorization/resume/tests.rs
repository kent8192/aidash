use super::*;
use crate::{
	Error,
	ports::authorization::resume::{WorkerResumeRepository, WorkerResumeScope},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

#[derive(Clone, Copy)]
struct Failure {
	stage: &'static str,
	retry: bool,
}
struct State {
	remote: bool,
	remote_denied: bool,
	attempts: VecDeque<Failure>,
	calls: Vec<String>,
	delays: Vec<Duration>,
	active: usize,
	leases: usize,
	pending: Option<&'static str>,
}
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	failure: Option<Failure>,
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		remote: false,
		remote_denied: false,
		attempts: VecDeque::new(),
		calls: vec![],
		delays: vec![],
		active: 0,
		leases: 0,
		pending: None,
	})))
}
impl Drop for Scope {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		state.active -= 1;
		state.calls.push("release".into());
	}
}
impl Scope {
	async fn step(&mut self, name: &'static str) -> Result<()> {
		let pending = {
			let mut state = self.state.lock().unwrap();
			state.calls.push(name.into());
			state.pending == Some(name)
		};
		if pending {
			std::future::pending::<()>().await;
		}
		if self.failure.is_some_and(|failure| failure.stage == name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
#[async_trait]
impl WorkerResumeRepository for Repository {
	fn remote(&self) -> bool {
		self.0.lock().unwrap().remote
	}
	async fn refresh_remote(&self) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		state.calls.push("remote_refresh".into());
		if state.remote_denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn lease(&self) -> Result<Box<dyn WorkerResumeScope + '_>> {
		let mut state = self.0.lock().unwrap();
		assert_eq!(state.active, 0);
		state.active = 1;
		state.leases += 1;
		state.calls.push("lease".into());
		Ok(Box::new(Scope {
			state: self.0.clone(),
			failure: state.attempts.pop_front(),
		}))
	}
	async fn wait(&self, delay: Duration) {
		let pending = {
			let mut state = self.0.lock().unwrap();
			assert_eq!(state.active, 0, "authority must be released before backoff");
			assert_eq!(state.calls.last().map(String::as_str), Some("release"));
			state.calls.push("wait".into());
			state.delays.push(delay);
			state.pending == Some("wait")
		};
		if pending {
			std::future::pending::<()>().await;
		}
	}
}
#[async_trait]
impl WorkerResumeScope for Scope {
	async fn refresh(&mut self) -> Result<()> {
		self.step("refresh").await
	}
	async fn guard(&mut self) -> Result<()> {
		self.step("guard").await
	}
	async fn inference(&mut self) -> Result<()> {
		self.step("inference").await
	}
	fn retryable(&self, _error: &Error) -> bool {
		self.failure.is_some_and(|failure| failure.retry)
	}
	async fn discard_failed_refresh(&mut self) {
		self.state.lock().unwrap().calls.push("discard".into());
	}
}
#[rstest]
#[tokio::test]
async fn successful_resume_rechecks_execution_and_inference_under_one_lease(
	repository: Repository,
) {
	resume(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		vec!["lease", "refresh", "guard", "inference", "release"]
	);
	assert_eq!(state.active, 0);
	assert!(state.delays.is_empty());
}
#[rstest]
#[case::refresh("refresh")]
#[case::guard("guard")]
#[case::inference("inference")]
#[tokio::test]
async fn a_transient_boundary_discards_failed_state_and_rechecks_every_stage_on_a_new_lease(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().attempts.push_back(Failure {
		stage: boundary,
		retry: true,
	});
	resume(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	let stages = ["refresh", "guard", "inference"];
	let last = stages.iter().position(|name| *name == boundary).unwrap();
	let mut expected = vec!["lease"];
	expected.extend(&stages[..=last]);
	expected.extend([
		"discard",
		"release",
		"wait",
		"lease",
		"refresh",
		"guard",
		"inference",
		"release",
	]);
	assert_eq!(state.calls, expected);
	assert_eq!(state.leases, 2);
	assert_eq!(state.active, 0);
	assert_eq!(state.delays, vec![Duration::from_millis(250)]);
}
#[rstest]
#[tokio::test]
async fn repeated_transient_failures_keep_the_existing_exponential_backoff_capped_at_two_seconds(
	repository: Repository,
) {
	repository.0.lock().unwrap().attempts.extend(
		[Failure {
			stage: "refresh",
			retry: true,
		}; 6],
	);
	resume(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.delays,
		[250, 500, 1000, 2000, 2000, 2000].map(Duration::from_millis)
	);
	assert_eq!(state.leases, 7);
	assert_eq!(
		state
			.calls
			.iter()
			.filter(|s| s.as_str() == "discard")
			.count(),
		6
	);
	assert_eq!(state.active, 0);
}
#[rstest]
#[case::refresh("refresh")]
#[case::guard("guard")]
#[case::inference("inference")]
#[tokio::test]
async fn a_permanent_fault_is_preserved_without_discarding_or_retrying_authority(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().attempts.push_back(Failure {
		stage: boundary,
		retry: false,
	});
	let Error::Port(error) = resume(&repository).await.err().unwrap() else {
		panic!("expected resume fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.leases, 1);
	assert_eq!(state.active, 0);
	assert!(!state.calls.contains(&"discard".into()));
	assert!(state.delays.is_empty());
}
#[rstest]
#[case::admitted(false)]
#[case::revoked(true)]
#[tokio::test]
async fn remote_resume_uses_current_admission_and_never_falls_back_to_local_scope(
	repository: Repository,
	#[case] denied: bool,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.remote = true;
		state.remote_denied = denied;
	}
	let result = resume(&repository).await;
	if denied {
		assert!(matches!(result, Err(Error::Forbidden)));
	} else {
		result.unwrap();
	}
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["remote_refresh"]);
	assert_eq!(state.leases, 0);
	assert!(state.delays.is_empty());
}
#[rstest]
#[case::refresh("refresh")]
#[case::inference("inference")]
#[tokio::test]
async fn cancelling_reacquisition_releases_the_current_authority_lease(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().pending = Some(boundary);
	assert!(
		tokio::time::timeout(Duration::from_millis(10), resume(&repository))
			.await
			.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert_eq!(state.calls.last().map(String::as_str), Some("release"));
	assert!(state.delays.is_empty());
}
#[rstest]
#[tokio::test]
async fn cancellation_during_backoff_holds_no_authority_transaction(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.pending = Some("wait");
		state.attempts.push_back(Failure {
			stage: "refresh",
			retry: true,
		});
	}
	assert!(
		tokio::time::timeout(Duration::from_millis(10), resume(&repository))
			.await
			.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert_eq!(
		state.calls,
		vec!["lease", "refresh", "discard", "release", "wait"]
	);
}

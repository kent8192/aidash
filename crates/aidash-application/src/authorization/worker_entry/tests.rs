use super::*;
use crate::Error;
use aidash_domain::{RawRun, context::Context};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};
use uuid::Uuid;

#[fixture]
fn run() -> Run {
	serde_json::from_value::<RawRun>(json!({
		"id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),
		"home_node":"aidash://home","agent_id":"agent","agent_version":"1",
		"phase":"READY","control":"ACTIVE","step":4,"revision":7,
		"observed_input_seq":9,"ledger_worker_ready":true,"error":null,
		"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z",
		"context":Context::default(),
		"pending":{"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":true}}
	}))
	.unwrap()
	.decode()
	.unwrap()
}
fn agent(files: bool) -> AgentConfig {
	let mut config: AgentConfig =
		serde_json::from_value(crate::test_support::agent("fixture").config).unwrap();
	config.core_capabilities = Default::default();
	config.core_capabilities.files = files;
	config
}

struct State {
	remote: bool,
	local: bool,
	files: bool,
	failure: Option<&'static str>,
	pending: Option<&'static str>,
	calls: Vec<&'static str>,
	alive: usize,
}
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	remote: bool,
}
impl Drop for Scope {
	fn drop(&mut self) {
		let mut state = self.state.lock().unwrap();
		state.alive -= 1;
		state.calls.push("drop");
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		remote: false,
		local: true,
		files: true,
		failure: None,
		pending: None,
		calls: vec![],
		alive: 0,
	})))
}
impl Repository {
	async fn step(&self, stage: &'static str) -> Result<()> {
		let (pending, failure) = {
			let mut state = self.0.lock().unwrap();
			state.calls.push(stage);
			(state.pending == Some(stage), state.failure == Some(stage))
		};
		if pending {
			std::future::pending::<()>().await;
		}
		if failure {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	fn scope(&self, remote: bool) -> Scope {
		self.0.lock().unwrap().alive += 1;
		Scope {
			state: self.0.clone(),
			remote,
		}
	}
}
#[async_trait]
impl WorkerEntryRepository<Scope> for Repository {
	async fn receiver_lease(&self, run: &RunMetadata) -> Result<Option<(Scope, AgentConfig)>> {
		assert_eq!(run.id, Uuid::from_u128(1));
		self.step("receiver").await?;
		let state = self.0.lock().unwrap();
		let (remote, files) = (state.remote, state.files);
		drop(state);
		Ok(remote.then(|| (self.scope(true), agent(files))))
	}
	async fn local_lease(&self, run: &RunMetadata, durable_audit: bool) -> Result<Option<Scope>> {
		assert_eq!(run.task_id, Uuid::from_u128(2));
		assert!(durable_audit);
		self.step("local").await?;
		let local = self.0.lock().unwrap().local;
		Ok(local.then(|| self.scope(false)))
	}
	async fn authorize(
		&self,
		scope: &mut Scope,
		run: &RunMetadata,
		read_context: bool,
	) -> Result<AgentConfig> {
		assert!(!scope.remote);
		assert_eq!(run.workspace_id, Uuid::from_u128(3));
		self.step(if read_context {
			"authorize_execution"
		} else {
			"authorize_delivery"
		})
		.await?;
		Ok(agent(self.0.lock().unwrap().files))
	}
	async fn initialize(&self, scope: &Scope, run: &Run, config: &AgentConfig) -> Result<()> {
		assert!(!scope.remote);
		assert_eq!(run.revision, 7);
		assert!(config.core_capabilities.enabled());
		assert_eq!(self.0.lock().unwrap().alive, 1);
		self.step("initialize").await
	}
}
#[rstest]
#[case::enabled(true, RunControl::Active, true)]
#[case::disabled(false, RunControl::Active, false)]
#[case::paused(true, RunControl::Paused, true)]
#[case::cancelled(true, RunControl::Cancelled, false)]
#[tokio::test]
async fn local_execution_authorizes_before_optional_initialization(
	repository: Repository,
	mut run: Run,
	#[case] files: bool,
	#[case] control: RunControl,
	#[case] initialized: bool,
) {
	repository.0.lock().unwrap().files = files;
	run.control = control;
	let entry = execution(&repository, &run).await.unwrap().unwrap();
	assert!(!entry.remote);
	assert_eq!(entry.agent.core_capabilities.enabled(), files);
	let mut expected = vec!["receiver", "local", "authorize_execution"];
	if initialized {
		expected.push("initialize");
	}
	assert_eq!(repository.0.lock().unwrap().calls, expected);
	assert_eq!(repository.0.lock().unwrap().alive, 1);
	drop(entry);
	assert_eq!(repository.0.lock().unwrap().alive, 0);
}
#[rstest]
#[case::execution(false)]
#[case::delivery(true)]
#[tokio::test]
async fn scoped_receivers_keep_their_agent_without_entering_the_local_path(
	repository: Repository,
	run: Run,
	#[case] delivery_only: bool,
) {
	repository.0.lock().unwrap().remote = true;
	let entry = if delivery_only {
		delivery(&repository, &run.metadata()).await
	} else {
		execution(&repository, &run).await
	}
	.unwrap()
	.unwrap();
	assert!(entry.remote);
	assert!(entry.scope.remote);
	assert_eq!(repository.0.lock().unwrap().calls, vec!["receiver"]);
	drop(entry);
	assert_eq!(repository.0.lock().unwrap().alive, 0);
}
#[rstest]
#[case::execution(false)]
#[case::delivery(true)]
#[tokio::test]
async fn unscoped_legacy_runs_do_not_gain_a_new_local_authority(
	repository: Repository,
	run: Run,
	#[case] delivery_only: bool,
) {
	repository.0.lock().unwrap().local = false;
	let entry = if delivery_only {
		delivery(&repository, &run.metadata()).await
	} else {
		execution(&repository, &run).await
	}
	.unwrap();
	assert!(entry.is_none());
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, vec!["receiver", "local"]);
	assert_eq!(state.alive, 0);
}
#[rstest]
#[tokio::test]
async fn delivery_checks_current_authority_without_decoding_context_or_initializing_core(
	repository: Repository,
	run: Run,
) {
	let entry = delivery(&repository, &run.metadata())
		.await
		.unwrap()
		.unwrap();
	assert!(!entry.remote);
	assert_eq!(
		repository.0.lock().unwrap().calls,
		vec!["receiver", "local", "authorize_delivery"]
	);
	drop(entry);
	assert_eq!(repository.0.lock().unwrap().alive, 0);
}
#[rstest]
#[case::receiver("receiver", 0)]
#[case::local("local", 1)]
#[case::guard("authorize_execution", 2)]
#[case::initialization("initialize", 3)]
#[tokio::test]
async fn every_entry_denial_stops_before_effects_and_releases_owned_authority(
	repository: Repository,
	run: Run,
	#[case] failure: &'static str,
	#[case] last: usize,
) {
	repository.0.lock().unwrap().failure = Some(failure);
	assert!(matches!(
		execution(&repository, &run).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	let mut expected =
		vec!["receiver", "local", "authorize_execution", "initialize"][..=last].to_vec();
	if last >= 2 {
		expected.push("drop");
	}
	assert_eq!(state.calls, expected);
	assert_eq!(state.alive, 0);
}
#[rstest]
#[tokio::test]
async fn a_receiver_error_never_falls_back_to_local_delivery(repository: Repository, run: Run) {
	repository.0.lock().unwrap().failure = Some("receiver");
	assert!(matches!(
		delivery(&repository, &run.metadata()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.0.lock().unwrap().calls, vec!["receiver"]);
}
#[rstest]
#[case::guard("authorize_execution")]
#[case::initialization("initialize")]
#[tokio::test]
async fn cancellation_drops_the_authority_even_when_an_entry_call_is_pending(
	repository: Repository,
	run: Run,
	#[case] pending: &'static str,
) {
	repository.0.lock().unwrap().pending = Some(pending);
	assert!(
		tokio::time::timeout(Duration::from_millis(10), execution(&repository, &run))
			.await
			.is_err()
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.alive, 0);
	assert_eq!(state.calls.last(), Some(&"drop"));
}

use super::*;
use crate::ports::capabilities::outbound::{AuthorizedRun, FetchPolicy, OutboundScope};
use aidash_domain::capabilities::outbound::failure_disclosure;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Fetch {
	Ready,
	Limit,
	Rejected,
	Pending,
}
struct State {
	record: Record,
	calls: Vec<String>,
	failure: &'static str,
	fetch: Fetch,
	fetched: bool,
	scopes: usize,
	failures: Vec<String>,
	load_state: Option<&'static str>,
}
#[derive(Clone)]
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	pending: Option<Record>,
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.state.lock().unwrap().scopes -= 1;
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		record: Record {
			id: Uuid::from_u128(1),
			tenant: "tenant".into(),
			owner: "owner".into(),
			area_id: Some(Uuid::from_u128(2)),
			kind: "outbound".into(),
			state: "approved".into(),
			revision: 1,
			data: json!({"url":"https://example.com/input","targets":["https://example.com"],"credential_id":Uuid::from_u128(3)}),
			expires_at: None,
		},
		calls: vec![],
		failure: "",
		fetch: Fetch::Ready,
		fetched: false,
		scopes: 0,
		failures: vec![],
		load_state: None,
	})))
}
#[async_trait]
impl OutboundRepository for Repository {
	fn fetch_policy(&self) -> FetchPolicy {
		FetchPolicy {
			origins: vec!["https://example.com".into()],
			output_bytes: 64,
		}
	}
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		Ok(vec![self.0.lock().unwrap().record.id])
	}
	async fn snapshot(&self, id: Uuid) -> Result<Record> {
		let mut state = self.0.lock().unwrap();
		assert_eq!(id, state.record.id);
		state.calls.push("snapshot".into());
		if state.failure == "snapshot" {
			Err(Error::External("snapshot failed".into()))
		} else {
			Ok(state.record.clone())
		}
	}
	async fn begin(&self, record: &Record) -> Result<Box<dyn OutboundScope + '_>> {
		let mut state = self.0.lock().unwrap();
		assert_eq!(record.owner, "owner");
		assert_eq!(record.tenant, "tenant");
		state.calls.push("begin".into());
		if state.failure == "begin" || (state.failure == "begin_after_fetch" && state.fetched) {
			return Err(Error::Forbidden);
		}
		state.scopes += 1;
		Ok(Box::new(Scope {
			state: self.0.clone(),
			pending: None,
		}))
	}
	async fn fail(&self, id: Uuid, message: &str) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		assert_eq!(id, state.record.id);
		state.calls.push(format!("fail:{message}"));
		state.failures.push(message.into());
		if state.failure == "fail" {
			return Err(Error::External("uncertain update failed".into()));
		}
		if matches!(state.record.state.as_str(), "approved" | "attempted") {
			state.record.state = "uncertain".into();
			state.record.data["error"] = failure_disclosure(message)["error"].clone();
		}
		Ok(())
	}
}
#[async_trait]
impl OutboundTransport for Repository {
	async fn fetch(
		&self,
		url: &str,
		targets: &Value,
		policy: &FetchPolicy,
	) -> Result<(u16, Vec<u8>, String)> {
		assert_eq!(url, "https://example.com/input");
		assert_eq!(targets, &json!(["https://example.com"]));
		assert_eq!(policy.origins, vec!["https://example.com"]);
		assert_eq!(policy.output_bytes, 64);
		let mode = {
			let mut state = self.0.lock().unwrap();
			state.calls.push("fetch".into());
			assert_eq!(state.record.state, "attempted");
			state.fetched = true;
			state.fetch
		};
		match mode {
			Fetch::Ready => Ok((201, b"hello".to_vec(), "https://example.com/final".into())),
			Fetch::Limit => Err(Error::Invalid("OUTBOUND_RESPONSE_LIMIT".into())),
			Fetch::Rejected => Err(Error::External("transport rejected".into())),
			Fetch::Pending => std::future::pending().await,
		}
	}
}
#[async_trait]
impl OutboundScope for Scope {
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(id, state.record.id);
		state.calls.push("load".into());
		let mut record = state.record.clone();
		if let Some(current) = state.load_state {
			record.state = current.into();
		}
		Ok(record)
	}
	async fn authorize(&mut self, record: &Record) -> Result<AuthorizedRun> {
		let mut state = self.state.lock().unwrap();
		state.calls.push(format!("authorize:{}", record.state));
		if state.failure == "admission_authorize"
			|| ((state.failure == "publish_authorize" || state.failure == "monitor_authorize")
				&& state.fetched)
		{
			return Err(Error::Forbidden);
		}
		Ok(AuthorizedRun {
			id: Uuid::from_u128(4),
			workspace_id: Uuid::from_u128(5),
		})
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		{
			let mut state = self.state.lock().unwrap();
			state.calls.push(format!("update:{}", record.state));
			if state.failure == "update" && record.state == "completed" {
				return Err(Error::External("update failed".into()));
			}
		}
		record.revision += 1;
		self.pending = Some(record.clone());
		Ok(())
	}
	async fn put(&mut self, area: Option<Uuid>, bytes: &[u8]) -> Result<(Uuid, String)> {
		let mut state = self.state.lock().unwrap();
		assert_eq!(area, state.record.area_id);
		assert_eq!(bytes, b"hello");
		state.calls.push("put".into());
		if state.failure == "put" {
			Err(Error::External("put failed".into()))
		} else {
			Ok((Uuid::from_u128(6), "digest".into()))
		}
	}
	async fn event(&mut self, run: &AuthorizedRun, id: Uuid, status: u16) -> Result<()> {
		assert_eq!(run.id, Uuid::from_u128(4));
		assert_eq!(run.workspace_id, Uuid::from_u128(5));
		assert_eq!(id, Uuid::from_u128(1));
		assert_eq!(status, 201);
		let mut state = self.state.lock().unwrap();
		state.calls.push("event".into());
		if state.failure == "event" {
			Err(Error::External("event failed".into()))
		} else {
			Ok(())
		}
	}
	async fn finish(self: Box<Self>, result: Result<Option<Record>>) -> Result<Option<Record>> {
		let mut state = self.state.lock().unwrap();
		if result.is_err() {
			state.calls.push("rollback".into());
			return result;
		}
		let phase = self
			.pending
			.as_ref()
			.map_or("check", |record| record.state.as_str());
		state.calls.push(format!("finish:{phase}"));
		if (state.failure == "admission_commit" && phase == "attempted")
			|| (state.failure == "finish" && phase == "completed")
		{
			return Err(Error::External(format!("{phase} commit failed")));
		}
		if let Some(record) = &self.pending {
			state.record = record.clone();
		}
		result
	}
}
async fn execute(repository: &Repository) -> Result<()> {
	drive(repository, repository, Uuid::from_u128(1)).await
}

#[rstest]
#[tokio::test]
async fn committed_intent_precedes_network_io_and_publication_remains_one_owned_scope(
	repository: Repository,
) {
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	let find = |value: &str| state.calls.iter().position(|call| call == value).unwrap();
	assert!(find("authorize:attempted") < find("update:attempted"));
	assert!(find("update:attempted") < find("finish:attempted"));
	assert!(find("finish:attempted") < find("fetch"));
	assert!(find("fetch") < find("put"));
	assert!(find("put") < find("update:completed"));
	assert!(find("update:completed") < find("event"));
	assert!(find("event") < find("finish:completed"));
	assert_eq!(state.record.state, "completed");
	assert_eq!(state.record.revision, 3);
	assert_eq!(state.record.data["output_file"]["size"], 5);
	assert_eq!(state.record.data["http_status"], 201);
	assert_eq!(state.scopes, 0);
	assert!(state.failures.is_empty());
}

#[rstest]
#[tokio::test]
async fn an_attempted_snapshot_is_never_dispatched_again(repository: Repository) {
	repository.0.lock().unwrap().record.state = "attempted".into();
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(
		state.calls,
		vec!["snapshot", "fail:OUTBOUND_EFFECT_UNCERTAIN"]
	);
	assert_eq!(state.record.state, "uncertain");
	assert!(!state.fetched);
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[case::completed("completed")]
#[case::revoked("revoked")]
#[case::uncertain("uncertain")]
#[tokio::test]
async fn a_freshly_loaded_nonapproved_record_is_not_sent(
	repository: Repository,
	#[case] current: &'static str,
) {
	repository.0.lock().unwrap().load_state = Some(current);
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert!(!state.fetched);
	assert!(state.failures.is_empty());
	assert_eq!(state.scopes, 0);
	assert!(state.calls.contains(&"finish:check".into()));
}

#[rstest]
#[case::authorization("admission_authorize")]
#[case::commit("admission_commit")]
#[tokio::test]
async fn failed_admission_or_commit_prevents_all_external_effects(
	repository: Repository,
	#[case] failure: &'static str,
) {
	repository.0.lock().unwrap().failure = failure;
	assert!(execute(&repository).await.is_err());
	let state = repository.0.lock().unwrap();
	assert!(!state.fetched);
	assert_eq!(state.record.state, "approved");
	assert_eq!(state.record.revision, 1);
	assert_eq!(state.scopes, 0);
	assert!(!state.calls.contains(&"put".into()));
}

#[rstest]
#[case::response_limit(Fetch::Limit, "OUTBOUND_RESPONSE_LIMIT")]
#[case::transport(Fetch::Rejected, "OUTBOUND_STOPPED")]
#[tokio::test]
async fn fetch_failures_retain_the_existing_uncertain_codes_without_partial_output(
	repository: Repository,
	#[case] mode: Fetch,
	#[case] code: &str,
) {
	repository.0.lock().unwrap().fetch = mode;
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.failures, vec![code]);
	assert_eq!(state.record.state, "uncertain");
	assert!(state.record.data["output_file"].is_null());
	assert!(!state.calls.contains(&"put".into()));
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn a_malformed_stored_url_stops_the_committed_attempt_before_transport(
	repository: Repository,
) {
	repository.0.lock().unwrap().record.data["url"] = Value::Null;
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.failures, vec!["OUTBOUND_STOPPED"]);
	assert!(!state.fetched);
	assert_eq!(state.record.state, "uncertain");
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[case::write("put")]
#[case::update("update")]
#[case::event("event")]
#[case::commit("finish")]
#[tokio::test]
async fn publication_failures_roll_back_and_leave_the_attempt_retryable_for_uncertainty_recovery(
	repository: Repository,
	#[case] failure: &'static str,
) {
	repository.0.lock().unwrap().failure = failure;
	assert!(execute(&repository).await.is_err());
	let state = repository.0.lock().unwrap();
	assert!(state.fetched);
	assert_eq!(state.record.state, "attempted");
	assert_eq!(state.record.revision, 2);
	assert!(state.record.data["output_file"].is_null());
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test]
async fn authority_is_checked_again_after_fetch_before_any_output_is_saved(repository: Repository) {
	repository.0.lock().unwrap().failure = "publish_authorize";
	assert!(matches!(execute(&repository).await, Err(Error::Forbidden)));
	let state = repository.0.lock().unwrap();
	assert!(state.fetched);
	assert!(!state.calls.contains(&"put".into()));
	assert_eq!(state.record.state, "attempted");
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[case::authorization("monitor_authorize")]
#[case::scope_creation("begin_after_fetch")]
#[tokio::test(start_paused = true)]
async fn withdrawn_authority_cancels_the_pending_channel_and_does_not_publish(
	repository: Repository,
	#[case] failure: &'static str,
) {
	{
		let mut state = repository.0.lock().unwrap();
		state.failure = failure;
		state.fetch = Fetch::Pending;
	}
	execute(&repository).await.unwrap();
	let state = repository.0.lock().unwrap();
	assert_eq!(state.failures, vec!["OUTBOUND_STOPPED"]);
	assert!(state.fetched);
	assert!(!state.calls.contains(&"put".into()));
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn the_sixty_second_timeout_keeps_the_original_outer_recovery_classification(
	repository: Repository,
) {
	repository.0.lock().unwrap().fetch = Fetch::Pending;
	let before = tokio::time::Instant::now();
	let error = execute(&repository).await.unwrap_err();
	assert!(matches!(error,Error::External(message) if message=="outbound timeout"));
	assert_eq!(before.elapsed(), StdDuration::from_secs(60));
	let state = repository.0.lock().unwrap();
	assert_eq!(state.record.state, "attempted");
	assert!(state.failures.is_empty());
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn fatal_drive_errors_use_the_existing_worker_fallback_without_repeating_io(
	repository: Repository,
) {
	repository.0.lock().unwrap().fetch = Fetch::Pending;
	reconcile(&repository, &repository, Uuid::from_u128(1)).await;
	let state = repository.0.lock().unwrap();
	assert_eq!(state.failures, vec!["AUTHORITY_WITHDRAWN"]);
	assert_eq!(state.record.state, "uncertain");
	assert_eq!(
		state.calls.iter().filter(|call| *call == "fetch").count(),
		1
	);
	assert_eq!(state.scopes, 0);
}

#[rstest]
#[case::snapshot("snapshot")]
#[case::begin("begin")]
#[tokio::test]
async fn snapshot_and_identity_scope_errors_are_preserved_before_any_network_call(
	repository: Repository,
	#[case] failure: &'static str,
) {
	repository.0.lock().unwrap().failure = failure;
	assert!(execute(&repository).await.is_err());
	let state = repository.0.lock().unwrap();
	assert!(!state.fetched);
	assert_eq!(state.scopes, 0);
	assert_eq!(state.record.state, "approved");
}

#[rstest]
#[tokio::test]
async fn a_failed_uncertain_update_is_returned_without_reporting_success(repository: Repository) {
	{
		let mut state = repository.0.lock().unwrap();
		state.record.state = "attempted".into();
		state.failure = "fail";
	}
	assert!(
		matches!(execute(&repository).await,Err(Error::External(message)) if message=="uncertain update failed")
	);
	let state = repository.0.lock().unwrap();
	assert!(!state.fetched);
	assert_eq!(state.record.state, "attempted");
	assert_eq!(state.scopes, 0);
}

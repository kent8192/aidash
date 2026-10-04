use super::*;
use crate::ports::registry::workbench::sandbox::SandboxScope;
use aidash_domain::registry::{
	EntityRef, Entry,
	workbench::{Draft, profile::TestProfile, sandbox::TestLimits},
};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use std::sync::{Arc, Mutex};

struct State {
	active: bool,
	phase: usize,
	commits: usize,
	rollbacks: usize,
	prepared: usize,
	sent: usize,
	calls: Option<Value>,
	order: Vec<String>,
}
struct Repository {
	state: Arc<Mutex<State>>,
	failure: Option<(usize, &'static str)>,
	response: &'static str,
}
impl Repository {
	fn new() -> Self {
		Self {
			state: Arc::new(Mutex::new(State {
				active: false,
				phase: 0,
				commits: 0,
				rollbacks: 0,
				prepared: 0,
				sent: 0,
				calls: Some(json!([{"id":"previous","outcome":"real","result":{"previous":true}}])),
				order: vec![],
			})),
			failure: None,
			response: "real",
		}
	}
	fn failing(&self, name: &str) -> bool {
		self.failure.is_some_and(|(phase, failure)| {
			phase == self.state.lock().unwrap().phase && failure == name
		})
	}
	fn dispatch(&self) -> Dispatch<'_> {
		Dispatch {
			repository: self,
			credentials: self,
			configuration: self,
			transport: self,
		}
	}
}
struct Scope<'a> {
	repository: &'a Repository,
	pending: Option<Value>,
	settled: bool,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		let mut state = self.repository.state.lock().unwrap();
		if !self.settled {
			state.rollbacks += 1;
		}
		state.active = false;
	}
}
fn call() -> ToolCall {
	ToolCall {
		id: "call".into(),
		name: "plugin_0".into(),
		arguments: json!({"action":"read","resource":"fixture"}),
	}
}
fn rule() -> RealToolRule {
	RealToolRule {
		tool: EntityRef {
			id: "tool".into(),
			version: "1".into(),
		},
		endpoint: "https://test.example/rpc".into(),
		credential_env: Some("TEST_TOOL".into()),
		allowed_actions: vec!["read".into()],
		allowed_resources: vec!["fixture".into()],
	}
}
fn pin(repository: &Repository) -> ProfilePin {
	ProfilePin {
		id: "profile".into(),
		revision: 7,
		tenant: "tenant".into(),
		rules: vec![rule()],
		credential_fingerprints: [(
			"TEST_TOOL".into(),
			credential_fingerprint(repository, "TEST_TOOL").unwrap(),
		)]
		.into(),
	}
}
fn session() -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 7,
		status: "running".into(),
		scenario: json!({}),
		conversation: Some(json!([])),
		tool_calls: None,
		usage: json!({}),
		error: None,
		created_at: Utc.timestamp_opt(100, 0).unwrap(),
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		expires_at: Utc.timestamp_opt(200, 0).unwrap(),
		expired_at: None,
	}
}
#[async_trait]
impl RealDispatchRepository for Repository {
	async fn begin_real(&self) -> Result<Box<dyn RealDispatchScope + '_>> {
		let mut state = self.state.lock().unwrap();
		assert!(!state.active);
		state.active = true;
		state.phase += 1;
		Ok(Box::new(Scope {
			repository: self,
			pending: None,
			settled: false,
		}))
	}
}
#[async_trait]
impl SandboxScope for Scope<'_> {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		assert_eq!(id, Uuid::from_u128(1));
		assert!(lock);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("draft_update_lock".into());
		Ok(Draft {
			id,
			tenant: "tenant".into(),
			owner: "owner".into(),
			revision: if self.repository.failing("draft_revision") {
				8
			} else {
				7
			},
			entry: json!({}),
			documents: json!([]),
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			archived: self.repository.failing("archived"),
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn authorize_draft(&mut self, _: &Draft, action: &str, shares: bool) -> Result<()> {
		assert_eq!(action, "agent_draft.test");
		assert!(shares);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("current_authority".into());
		if self.repository.failing("authority") {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn session(&mut self, id: Uuid, lock: bool) -> Result<TestSession> {
		assert_eq!(id, Uuid::from_u128(9));
		let mut row = session();
		row.tool_calls = self.repository.state.lock().unwrap().calls.clone();
		if self.repository.failing("stopped") {
			row.status = "stopped".into();
		}
		self.repository.state.lock().unwrap().order.push(
			if lock {
				"session_update_lock"
			} else {
				"session_reload"
			}
			.into(),
		);
		Ok(row)
	}
	async fn limits(&mut self, _: &str) -> Result<TestLimits> {
		panic!("dispatch must not change limits")
	}
	async fn save_limits(&mut self, _: &TestLimits) -> Result<()> {
		panic!("dispatch must not change limits")
	}
	async fn sessions(&mut self, _: Uuid) -> Result<Vec<TestSession>> {
		panic!("dispatch must not page sessions")
	}
	async fn stop(&mut self, _: Uuid) -> Result<TestSession> {
		panic!("dispatch must not stop sessions")
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		let mut state = self.repository.state.lock().unwrap();
		if let Some(calls) = self.pending.take() {
			state.calls = Some(calls);
		}
		state.commits += 1;
		state.order.push("commit".into());
		self.settled = true;
		Ok(())
	}
}
#[async_trait]
impl RealDispatchScope for Scope<'_> {
	async fn lock_identity(&mut self) -> Result<()> {
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("identity_lock".into());
		Ok(())
	}
	async fn profile(&mut self, tenant: &str, id: &str) -> Result<TestProfile> {
		assert_eq!((tenant, id), ("tenant", "profile"));
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("profile_share_lock".into());
		Ok(TestProfile {
			tenant: tenant.into(),
			id: id.into(),
			revision: if self.repository.failing("profile_revision") {
				8
			} else {
				7
			},
			enabled: !self.repository.failing("disabled"),
			rules: if self.repository.failing("rules") {
				json!([])
			} else {
				json!([rule()])
			},
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry> {
		assert_eq!(*reference, rule().tool);
		self.repository
			.state
			.lock()
			.unwrap()
			.order
			.push("effective_tool".into());
		Ok(serde_json::from_value(json!({
			"id":"tool","version":"1","kind":"tool","name":{},"description":{},
			"schema": if self.repository.failing("schema") { json!({"required":["extra"]}) } else { json!({"type":"object","required":["action","resource"]}) },
			"config":{"transport":"http","endpoint":"https://production.example/rpc","credential_env":"PRODUCTION_TOOL","replay":"read_only"}
		}))?)
	}
	async fn write_calls(&mut self, id: Uuid, calls: Value, running: bool) -> Result<()> {
		assert_eq!(id, Uuid::from_u128(9));
		let mut state = self.repository.state.lock().unwrap();
		assert_eq!(running, state.phase == 1);
		state.order.push(
			if running {
				"persist_pending"
			} else {
				"persist_result"
			}
			.into(),
		);
		self.pending = Some(calls);
		Ok(())
	}
	async fn rollback(mut self: Box<Self>) -> Result<()> {
		self.repository.state.lock().unwrap().rollbacks += 1;
		self.settled = true;
		Ok(())
	}
}
impl Credentials for Repository {
	fn resolve(&self, name: &str) -> Result<String> {
		assert_eq!(name, "TEST_TOOL");
		Ok(if self.failing("credential") {
			"rotated-fixture"
		} else {
			"fixture"
		}
		.into())
	}
}
impl ProfileConfiguration for Repository {
	fn require_secret(&self, name: &str) -> Result<()> {
		assert_eq!(name, "TEST_TOOL");
		if self.failing("configuration") {
			Err(Error::Invalid("test credential missing".into()))
		} else {
			Ok(())
		}
	}
}
struct Request {
	state: Arc<Mutex<State>>,
	response: &'static str,
}
impl RealToolTransport for Repository {
	fn prepare(
		&self,
		id: Uuid,
		selected: &RealToolRule,
		input: &ToolCall,
	) -> Result<Box<dyn PreparedRealRequest>> {
		assert_eq!(id, Uuid::from_u128(9));
		assert_eq!(selected.endpoint, rule().endpoint);
		assert_eq!(*input, call());
		if self.failing("prepare") {
			return Err(Error::Invalid("transport cannot build request".into()));
		}
		self.state.lock().unwrap().prepared += 1;
		Ok(Box::new(Request {
			state: self.state.clone(),
			response: self.response,
		}))
	}
}
#[async_trait]
impl PreparedRealRequest for Request {
	async fn send(self: Box<Self>) -> Result<(Value, &'static str)> {
		{
			let mut state = self.state.lock().unwrap();
			assert!(state.active);
			assert_eq!(state.phase, 2);
			assert_eq!(state.commits, 1);
			assert_eq!(state.prepared, 2);
			assert_eq!(
				state.calls.as_ref().unwrap()[1]["outcome"],
				"outcome_unknown"
			);
			state.sent += 1;
			state.order.push("send".into());
			match self.response {
				"changed" => {
					state.calls.as_mut().unwrap()[1]["id"] = json!("other");
				}
				"lost" => {
					state.calls = None;
				}
				_ => {}
			}
		}
		match self.response {
			"network_error" => Err(Error::External("connection lost after dispatch".into())),
			"pending" => std::future::pending().await,
			"failed" => Ok((
				json!({"error":"test Tool returned an HTTP error","status":503}),
				"failed",
			)),
			_ => Ok((json!({"fixture":true}), "real")),
		}
	}
}

#[rstest]
#[case("real")]
#[case("failed")]
#[tokio::test]
async fn pending_is_durable_before_send_and_current_leases_cover_result(
	#[case] outcome: &'static str,
) {
	// Arrange a prior settled call so replacement cannot erase earlier evidence.
	let mut repository = Repository::new();
	repository.response = outcome;
	let pin = pin(&repository);
	// Act through both current-authority transactions.
	let (result, actual) = invoke(&repository.dispatch(), session().id, &pin, &rule(), &call())
		.await
		.unwrap();
	// Assert authority is independently re-read and only the matching marker settles.
	let state = repository.state.lock().unwrap();
	assert_eq!(actual, outcome);
	assert_eq!(
		(state.phase, state.commits, state.rollbacks, state.sent),
		(2, 2, 0, 1)
	);
	assert!(!state.active);
	assert_eq!(
		state.calls.as_ref().unwrap()[0],
		json!({"id":"previous","outcome":"real","result":{"previous":true}})
	);
	assert_eq!(
		state.calls.as_ref().unwrap()[1],
		json!({"id":"call","name":"plugin_0","arguments":call().arguments,"outcome":outcome,"result":result,"endpoint":rule().endpoint})
	);
	assert_eq!(
		state.order,
		[
			"session_update_lock",
			"identity_lock",
			"draft_update_lock",
			"current_authority",
			"profile_share_lock",
			"effective_tool",
			"persist_pending",
			"commit",
			"session_update_lock",
			"identity_lock",
			"draft_update_lock",
			"current_authority",
			"profile_share_lock",
			"effective_tool",
			"send",
			"session_reload",
			"persist_result",
			"commit",
		]
	);
}
#[rstest]
#[case("action", json!("write"))]
#[case("resource", json!("other"))]
#[case("action", json!(null))]
#[case("resource", json!(17))]
#[tokio::test]
async fn invalid_or_unlisted_arguments_never_open_a_transaction(
	#[case] field: &str,
	#[case] value: Value,
) {
	let repository = Repository::new();
	let pin = pin(&repository);
	let mut input = call();
	input.arguments[field] = value;
	assert!(
		invoke(&repository.dispatch(), session().id, &pin, &rule(), &input)
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert_eq!((state.phase, state.commits, state.sent), (0, 0, 0));
}
#[rstest]
#[case("stopped")]
#[case("draft_revision")]
#[case("archived")]
#[case("authority")]
#[case("profile_revision")]
#[case("disabled")]
#[case("rules")]
#[case("credential")]
#[case("schema")]
#[case("configuration")]
#[case("prepare")]
#[tokio::test]
async fn invalid_initial_authority_or_request_leaves_no_new_marker(#[case] failure: &'static str) {
	let mut repository = Repository::new();
	let pin = pin(&repository);
	repository.failure = Some((1, failure));
	assert!(
		invoke(&repository.dispatch(), session().id, &pin, &rule(), &call())
			.await
			.is_err()
	);
	let state = repository.state.lock().unwrap();
	assert_eq!(
		(state.phase, state.commits, state.rollbacks, state.sent),
		(1, 0, 1, 0)
	);
	assert_eq!(state.calls.as_ref().unwrap().as_array().unwrap().len(), 1);
	assert!(!state.active);
}
#[rstest]
#[case("authority")]
#[case("stopped")]
#[case("draft_revision")]
#[case("disabled")]
#[case("rules")]
#[case("credential")]
#[case("schema")]
#[case("prepare")]
#[tokio::test]
async fn changed_authority_after_marker_records_denial_without_sending(
	#[case] failure: &'static str,
) {
	let mut repository = Repository::new();
	let pin = pin(&repository);
	repository.failure = Some((2, failure));
	let error = invoke(&repository.dispatch(), session().id, &pin, &rule(), &call())
		.await
		.unwrap_err();
	let state = repository.state.lock().unwrap();
	assert_eq!(
		(state.phase, state.commits, state.rollbacks, state.sent),
		(3, 2, 1, 0)
	);
	assert_eq!(state.calls.as_ref().unwrap()[1]["outcome"], "denied");
	assert_eq!(state.calls.as_ref().unwrap()[1]["error"], error.to_string());
	assert!(!state.active);
}
#[tokio::test]
async fn transport_error_preserves_unknown_durable_outcome() {
	let mut repository = Repository::new();
	repository.response = "network_error";
	let pin = pin(&repository);
	assert!(matches!(
		invoke(&repository.dispatch(), session().id, &pin, &rule(), &call()).await,
		Err(Error::External(_))
	));
	let state = repository.state.lock().unwrap();
	assert_eq!((state.commits, state.rollbacks, state.sent), (1, 1, 1));
	assert_eq!(
		state.calls.as_ref().unwrap()[1]["outcome"],
		"outcome_unknown"
	);
	assert!(!state.active);
}
#[tokio::test]
async fn cancellation_during_dispatch_releases_leases_and_preserves_unknown_marker() {
	let mut repository = Repository::new();
	repository.response = "pending";
	let pin = pin(&repository);
	let dispatch = repository.dispatch();
	let rule = rule();
	let call = call();
	{
		let operation = invoke(&dispatch, session().id, &pin, &rule, &call);
		tokio::pin!(operation);
		assert!(futures_util::poll!(&mut operation).is_pending());
		assert!(repository.state.lock().unwrap().active);
	}
	let state = repository.state.lock().unwrap();
	assert_eq!((state.commits, state.rollbacks, state.sent), (1, 1, 1));
	assert_eq!(
		state.calls.as_ref().unwrap()[1]["outcome"],
		"outcome_unknown"
	);
	assert!(!state.active);
}
#[rstest]
#[case("changed")]
#[case("lost")]
#[tokio::test]
async fn result_never_overwrites_a_changed_or_lost_marker(#[case] response: &'static str) {
	let mut repository = Repository::new();
	repository.response = response;
	let pin = pin(&repository);
	assert!(matches!(
		invoke(&repository.dispatch(), session().id, &pin, &rule(), &call()).await,
		Err(Error::Conflict(_))
	));
	let state = repository.state.lock().unwrap();
	assert_eq!((state.commits, state.rollbacks, state.sent), (1, 1, 1));
	assert!(!state.active);
	if response == "changed" {
		assert_eq!(state.calls.as_ref().unwrap()[1]["id"], "other");
	} else {
		assert_eq!(state.calls, None);
	}
}

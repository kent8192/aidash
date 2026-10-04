use super::*;
use aidash_domain::{
	RawRun, RunControl, RunMetadata, RunPhase, context::Context, invocation::InvocationSummary,
	policy::Resource,
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
struct State {
	raw: Option<RawRun>,
	visible: bool,
	memory: Option<Value>,
	calls: Vec<String>,
	fail: Option<&'static str>,
	deny: bool,
	pending: bool,
	context: Option<Value>,
	offset: Option<u64>,
}
struct Repository(Arc<Mutex<State>>);
struct Scope {
	state: Arc<Mutex<State>>,
	finished: bool,
}
impl State {
	fn record(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.finished {
			self.state.lock().unwrap().calls.push("rollback".into());
		}
	}
}
#[fixture]
fn repository() -> Repository {
	Repository(Arc::new(Mutex::new(State {
		raw: Some(RawRun {
			metadata: RunMetadata {
				id: id(1),
				task_id: id(2),
				workspace_id: id(3),
				home_node: "aidash://local".into(),
				agent_id: "producer".into(),
				agent_version: "1".into(),
				phase: RunPhase::Ready,
				control: RunControl::Paused,
				step: 4,
				revision: 8,
				observed_input_seq: 9,
				ledger_worker_ready: true,
				error: None,
				lease_owner: None,
				lease_until: None,
				updated_at: Utc::now(),
			},
			context: serde_json::to_value(Context::default()).unwrap(),
			pending: json!({"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false}}),
		}),
		visible: true,
		memory: Some(json!({"saved":null})),
		calls: vec![],
		fail: None,
		deny: false,
		pending: false,
		context: None,
		offset: None,
	})))
}
#[async_trait]
impl RunDetailsRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn RunDetailsScope + '_>> {
		self.0.lock().unwrap().record("begin")?;
		Ok(Box::new(Scope {
			state: self.0.clone(),
			finished: false,
		}))
	}
}
#[async_trait]
impl RunDetailsScope for Scope {
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn set_context(&mut self, context: Value) {
		self.state.lock().unwrap().context = Some(context);
	}
	async fn run(&mut self, key: Uuid) -> Result<Option<RawRun>> {
		assert_eq!(key, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("run")?;
		Ok(state.raw.clone())
	}
	async fn workspace(&mut self, key: Uuid) -> Result<Resource> {
		assert_eq!(key, id(3));
		self.state.lock().unwrap().record("workspace")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: key.to_string(),
			attributes: json!({"current":true}),
		})
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(action, "workspace.read");
		assert_eq!(resource.kind, "workspace");
		let mut state = self.state.lock().unwrap();
		assert_eq!(state.context, Some(json!({"current":true})));
		state.record(action)?;
		if state.deny {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn run_visible(&mut self, run: &aidash_domain::RunInspection) -> Result<bool> {
		assert_eq!(run.id, id(1));
		let mut state = self.state.lock().unwrap();
		state.record("visible")?;
		Ok(state.visible)
	}
	async fn invocations(&mut self, key: Uuid, offset: u64) -> Result<Vec<InvocationSummary>> {
		assert_eq!(key, id(1));
		let pending = {
			let mut state = self.state.lock().unwrap();
			state.record("invocations")?;
			state.offset = Some(offset);
			state.pending
		};
		if pending {
			std::future::pending::<()>().await;
		}
		Ok(["first", "second"]
			.map(|key| InvocationSummary {
				idempotency_key: key.into(),
				run_id: id(1),
				tool: "saved tool".into(),
				input: json!({"truncated":true,"preview":"東京"}),
				status: "UNCERTAIN".into(),
				result: Some(Value::Null),
				replay_safe: false,
				created_at: Utc::now(),
			})
			.to_vec())
	}
	async fn memory(&mut self, run: &RunMetadata) -> Result<Option<Value>> {
		assert_eq!(run.agent_id, "producer");
		assert_eq!(run.agent_version, "1");
		assert_eq!(run.workspace_id, id(3));
		let mut state = self.state.lock().unwrap();
		state.record("memory")?;
		Ok(state.memory.clone())
	}
	async fn media_input_routes(&mut self, run: &RunMetadata) -> Result<Vec<Vec<String>>> {
		assert_eq!(run.id, id(1));
		self.state.lock().unwrap().record("routes")?;
		Ok(vec![
			vec!["image/png".into(), "audio/wav".into()],
			vec!["image/png".into()],
		])
	}
	async fn finish(mut self: Box<Self>, result: Result<RunDetails>) -> Result<RunDetails> {
		match result {
			Ok(details) => {
				self.state.lock().unwrap().record("commit")?;
				self.finished = true;
				Ok(details)
			}
			Err(error) => {
				self.state.lock().unwrap().calls.push("rollback".into());
				self.finished = true;
				Err(error)
			}
		}
	}
}
fn expected(local: bool) -> Vec<&'static str> {
	let mut calls = vec!["begin", "run"];
	if local {
		calls.extend(["workspace", "workspace.read"]);
	}
	calls.extend(["visible", "invocations", "memory", "routes", "commit"]);
	calls
}
#[rstest]
#[case::first_page(0)]
#[case::later_page(100)]
#[case::maximum_offset(u64::MAX)]
#[tokio::test]
async fn authorized_inspection_preserves_the_native_page_and_projection(
	repository: Repository,
	#[case] offset: u64,
) {
	let details = inspect(&repository, id(1), offset).await.unwrap();
	assert_eq!(details.run.id, id(1));
	assert_eq!(details.run.revision, 8);
	assert_eq!(details.memory, json!({"saved":null}));
	assert_eq!(
		details
			.invocations
			.iter()
			.map(|i| i.idempotency_key.as_str())
			.collect::<Vec<_>>(),
		vec!["first", "second"]
	);
	assert_eq!(
		details.invocations[0].input,
		json!({"truncated":true,"preview":"東京"})
	);
	assert_eq!(details.invocations[0].result, Some(Value::Null));
	assert_eq!(details.invocations[0].status, "UNCERTAIN");
	assert!(!details.invocations[0].replay_safe);
	assert_eq!(
		details.media_input_routes,
		vec![vec!["image/png", "audio/wav"], vec!["image/png"]]
	);
	let state = repository.0.lock().unwrap();
	assert_eq!(state.offset, Some(offset));
	assert_eq!(state.calls, expected(true));
}
#[rstest]
#[tokio::test]
async fn a_foreign_run_uses_its_run_visibility_instead_of_local_workspace_authority(
	repository: Repository,
) {
	repository
		.0
		.lock()
		.unwrap()
		.raw
		.as_mut()
		.unwrap()
		.metadata
		.home_node = "aidash://home".into();
	let details = inspect(&repository, id(1), 0).await.unwrap();
	assert_eq!(details.run.home_node, "aidash://home");
	let state = repository.0.lock().unwrap();
	assert_eq!(state.calls, expected(false));
	assert_eq!(state.context, None);
}
#[rstest]
#[case::missing(true)]
#[case::invisible(false)]
#[tokio::test]
async fn missing_or_invisible_runs_never_read_invocations_memory_or_routes(
	repository: Repository,
	#[case] missing: bool,
) {
	{
		let mut state = repository.0.lock().unwrap();
		if missing {
			state.raw = None;
		} else {
			state.visible = false;
		}
	}
	assert!(matches!(
		inspect(&repository, id(1), 0).await,
		Err(Error::Forbidden)
	));
	let state = repository.0.lock().unwrap();
	assert!(!state.calls.contains(&"invocations".into()));
	assert_eq!(state.calls.last().map(String::as_str), Some("rollback"));
}
#[rstest]
#[tokio::test]
async fn revoked_workspace_read_stops_before_run_disclosure(repository: Repository) {
	repository.0.lock().unwrap().deny = true;
	assert!(matches!(
		inspect(&repository, id(1), 0).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.0.lock().unwrap().calls,
		vec!["begin", "run", "workspace", "workspace.read", "rollback"]
	);
}
#[rstest]
#[case::missing(None,json!({}))]
#[case::explicit_null(Some(Value::Null), Value::Null)]
#[tokio::test]
async fn only_an_absent_memory_row_defaults_to_an_empty_object(
	repository: Repository,
	#[case] memory: Option<Value>,
	#[case] expected: Value,
) {
	repository.0.lock().unwrap().memory = memory;
	assert_eq!(
		inspect(&repository, id(1), 0).await.unwrap().memory,
		expected
	);
}
#[rstest]
#[tokio::test]
async fn corrupt_context_remains_a_diagnostic_after_current_disclosure_authorization(
	repository: Repository,
) {
	repository.0.lock().unwrap().raw.as_mut().unwrap().context = Value::Null;
	let details = inspect(&repository, id(1), 0).await.unwrap();
	assert!(details.run.context.is_none());
	assert_eq!(
		details.run.state_error.as_deref(),
		Some("invalid execution context")
	);
	assert_eq!(repository.0.lock().unwrap().calls, expected(true));
}
#[rstest]
#[case::begin("begin")]
#[case::run("run")]
#[case::workspace("workspace")]
#[case::read_authority("workspace.read")]
#[case::visibility("visible")]
#[case::invocations("invocations")]
#[case::memory("memory")]
#[case::route("routes")]
#[case::commit("commit")]
#[tokio::test]
async fn any_projection_fault_rolls_back_without_reading_later_data(
	repository: Repository,
	#[case] boundary: &'static str,
) {
	repository.0.lock().unwrap().fail = Some(boundary);
	let Error::Port(error) = inspect(&repository, id(1), 0).await.err().unwrap() else {
		panic!("expected projection fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	let state = repository.0.lock().unwrap();
	let expected = expected(true);
	let last = expected.iter().position(|s| *s == boundary).unwrap();
	assert_eq!(&state.calls[..=last], &expected[..=last]);
	assert_eq!(
		state.calls.len(),
		last + 1 + usize::from(boundary != "begin")
	);
}
#[rstest]
#[tokio::test]
async fn dropping_an_incomplete_projection_releases_its_disclosure_scope(repository: Repository) {
	repository.0.lock().unwrap().pending = true;
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			inspect(&repository, id(1), 0)
		)
		.await
		.is_err()
	);
	assert_eq!(
		repository
			.0
			.lock()
			.unwrap()
			.calls
			.last()
			.map(String::as_str),
		Some("rollback")
	);
}

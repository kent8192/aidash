use super::*;
use crate::ports::{
	catalog::CatalogScope, generation::compaction::remote::RemoteCompactionTransport,
};
use aidash_domain::{
	RunControl, Task, TaskStatus,
	context::Context,
	federation::execution::{Description, Inspection},
	generation::{
		dispatch::Input,
		remote::{Purpose, Usage},
	},
	policy::Resource,
	registry::{EntityRef, Entry},
	run_state::{RecoveryState, RunState, StateVersion, ThinkingState},
	semantic::remote::Provider,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

#[derive(Clone)]
struct World(Arc<Mutex<State>>);
struct State {
	run: Run,
	entry: Entry,
	description: Value,
	available: VecDeque<bool>,
	approved: bool,
	denied: bool,
	held: bool,
	events: Vec<String>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	factory_fails: bool,
	credential_fails: bool,
	oversized: bool,
	provider_fails: bool,
	input: Option<Input>,
	settled: usize,
}
fn reference() -> EntityRef {
	EntityRef {
		id: "compactor".into(),
		version: "1.0.0".into(),
	}
}
#[fixture]
fn world() -> World {
	let now = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
	let run = Run {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://home".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: StateVersion::default(),
		state: RunState::Thinking(ThinkingState::default()),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context: Context::default(),
		step: 7,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: now,
	};
	let entry:Entry=serde_json::from_value(json!({"id":"compactor","version":"1.0.0","kind":"compactor",
        "name":{"en":"Compactor"},"description":{},"config":{"provider":"typesafe","endpoint":"https://provider.example/v1",
        "model":"fixture","credential_env":"FIXTURE_KEY","max_request_bytes":2048,"max_questions":10,"max_response_bytes":1024}})).unwrap();
	let provider = Provider {
		node_id: "aidash://executor".into(),
		entry: reference(),
		digest: digest(&json!(entry)),
		configuration_digest: digest(&entry.config),
	};
	let description = Description {
		grant_id: Uuid::from_u128(4),
		source_node: run.home_node.clone(),
		target_node: "aidash://executor".into(),
		source_tenant: "tenant".into(),
		source_subject: "creator".into(),
		task: Task {
			id: run.task_id,
			workspace_id: run.workspace_id,
			title: "Task".into(),
			description: "Work".into(),
			status: TaskStatus::Running,
			requirements: json!({}),
			owner: None,
			created_by: "creator".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 1,
			created_at: now,
		},
		inspection: Inspection {
			generation: None,
			lineage: vec![],
			node_id: "aidash://executor".into(),
			authority_digest: digest(&json!({})),
			agent: entry.clone(),
			definitions: vec![],
			semantic_memory: 1,
			compactor: Some(reference()),
		},
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
		semantic: Binding::RequiredHome {
			native: None,
			home_lineage: vec![],
			execution_lineage: vec![],
			version: 1,
			index_revision: 1,
			index_digest: digest(&json!({})),
			embedding: Box::new(provider.clone()),
			compactor: Some(Box::new(provider)),
		},
	};
	World(Arc::new(Mutex::new(State {
		run,
		entry,
		description: json!(description),
		available: VecDeque::from([true, true]),
		approved: true,
		denied: false,
		held: true,
		events: vec![],
		failure: None,
		pause: None,
		factory_fails: false,
		credential_fails: false,
		oversized: false,
		provider_fails: false,
		input: None,
		settled: 0,
	})))
}
impl World {
	async fn point(&self, name: &'static str) -> Result<()> {
		let (fail, pause) = {
			let mut state = self.0.lock().unwrap();
			state.events.push(name.into());
			(state.failure == Some(name), state.pause == Some(name))
		};
		if fail {
			return Err(Error::External(format!("{name} failed")));
		}
		if pause {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
	fn run(&self) -> Run {
		self.0.lock().unwrap().run.clone()
	}
}
struct Authority(World);
#[async_trait]
impl RemoteCompactionAuthority for Authority {
	fn node_id(&self) -> &str {
		"aidash://executor"
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		self
	}
	async fn suspend(&mut self) -> Result<()> {
		self.0.point("suspend").await?;
		self.0.0.lock().unwrap().held = false;
		Ok(())
	}
	async fn refresh(&mut self, run: &Run) -> Result<bool> {
		assert_eq!(run.id, self.0.run().id);
		self.0.point("refresh").await?;
		let mut state = self.0.0.lock().unwrap();
		assert!(!state.held);
		let available = state.available.pop_front().unwrap();
		state.held = available;
		Ok(available)
	}
	async fn description(&mut self, id: Uuid) -> Result<Value> {
		self.0.point("description").await?;
		let state = self.0.0.lock().unwrap();
		assert!(state.held);
		assert_eq!(id, state.run.id);
		Ok(state.description.clone())
	}
	async fn admit(
		&mut self,
		run: &Run,
		attempt: Uuid,
		input_digest: String,
		amount: i64,
	) -> Result<Input> {
		self.0.point("admit").await?;
		let mut state = self.0.0.lock().unwrap();
		assert!(!state.held);
		assert!(!attempt.is_nil());
		let description: Description = serde_json::from_value(state.description.clone()).unwrap();
		let Binding::RequiredHome {
			compactor: Some(provider),
			..
		} = description.semantic
		else {
			panic!("binding")
		};
		let input = Input {
			usage: Usage {
				operation_id: attempt,
				attempt_id: attempt,
				dispatcher_node: "aidash://executor".into(),
				grant_id: description.grant_id,
				admission_id: run.id,
				purpose: Purpose::Compaction,
				provider: *provider,
				input_digest,
				reserved_tokens: amount,
			},
			boundary: json!({"step":run.step}),
		};
		state.input = Some(input.clone());
		Ok(input)
	}
	async fn settle(&mut self, input: &Input) -> Result<()> {
		self.0.point("settle").await?;
		let mut state = self.0.0.lock().unwrap();
		assert!(!state.held);
		assert_eq!(json!(input), json!(state.input.as_ref().unwrap()));
		state.settled += 1;
		Ok(())
	}
}
#[async_trait]
impl CatalogScope for Authority {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn inherited_lease(&self) -> bool {
		true
	}
	fn approved(&self, entry: &EntityRef) -> bool {
		assert_eq!(entry, &reference());
		self.0.0.lock().unwrap().approved
	}
	fn remember(&mut self, _: &EntityRef) {}
	async fn distribution_lock(&mut self) -> Result<()> {
		panic!("fresh worker lease is inherited")
	}
	async fn document(&mut self, _: &EntityRef) -> Result<Option<Value>> {
		self.0.point("catalog").await?;
		let state = self.0.0.lock().unwrap();
		assert!(state.held);
		Ok(Some(json!(state.entry)))
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		panic!("no discovery")
	}
	fn resource(&self, entry: &Entry) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: entry.kind.clone(),
			id: entry.id.clone(),
			attributes: json!({"version":entry.version}),
		}
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		assert_eq!(action, "compaction.invoke");
		self.0.point("compaction.invoke").await?;
		if self.0.0.lock().unwrap().denied {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		panic!("exact permission is required")
	}
	async fn active(&mut self, _: &Entry) -> Result<bool> {
		panic!("no discovery")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("the approved digests are checked explicitly")
	}
}
struct Factory(World);
struct Transport(World);
impl RemoteCompactionProvider for Factory {
	fn approved_remote(
		&self,
		config: CompactorConfig,
	) -> Result<Arc<dyn RemoteCompactionTransport>> {
		assert_eq!(config.max_request_bytes, 2048);
		let mut state = self.0.0.lock().unwrap();
		state.events.push("provider.approved".into());
		if state.factory_fails {
			return Err(Error::Invalid("invalid provider".into()));
		}
		Ok(Arc::new(Transport(self.0.clone())))
	}
}
#[async_trait]
impl RemoteCompactionTransport for Transport {
	fn check_credential(&self) -> Result<()> {
		let mut state = self.0.0.lock().unwrap();
		state.events.push("provider.credential".into());
		if state.credential_fails {
			return Err(Error::RemoteSemantic(Failure::Configuration));
		}
		Ok(())
	}
	fn check_request(&self, _: &Value, questions: &CompactionQuestions) -> Result<usize> {
		assert_eq!(questions.len(), 2);
		let mut state = self.0.0.lock().unwrap();
		state.events.push("provider.check".into());
		if state.oversized {
			return Err(Error::Invalid("oversized request".into()));
		}
		Ok(100)
	}
	async fn ask(&self, state: &Value, questions: &CompactionQuestions) -> Result<Value> {
		assert_eq!(state, &json!({"history":"bounded"}));
		assert_eq!(questions.len(), 2);
		{
			let data = self.0.0.lock().unwrap();
			assert!(!data.held);
			assert!(data.input.is_some());
		}
		self.0.point("provider.ask").await?;
		if self.0.0.lock().unwrap().provider_fails {
			return Err(Error::RemoteSemantic(Failure::Unavailable));
		}
		Ok(json!({"probabilities":[0.25,0.75]}))
	}
}
async fn invoke(world: &World) -> Result<Value> {
	let questions = json!({"retain":"Keep?","discard":"Drop?"});
	ask(
		&mut Authority(world.clone()),
		&Factory(world.clone()),
		&world.run(),
		&json!({"history":"bounded"}),
		questions.as_object().unwrap(),
	)
	.await
}
#[rstest]
#[tokio::test]
async fn approved_remote_call_preserves_exact_body_maximum_charge_and_lease_order(world: World) {
	assert_eq!(
		invoke(&world).await.unwrap(),
		json!({"probabilities":[0.25,0.75]})
	);
	let state = world.0.lock().unwrap();
	assert_eq!(
		state.events,
		[
			"suspend",
			"refresh",
			"description",
			"catalog",
			"compaction.invoke",
			"provider.approved",
			"provider.credential",
			"provider.check",
			"suspend",
			"admit",
			"provider.ask",
			"settle",
			"refresh"
		]
	);
	let input = state.input.as_ref().unwrap();
	assert_eq!(input.usage.reserved_tokens, 3072);
	assert_eq!(state.settled, 1);
	assert!(state.held);
	assert_eq!(
		input.usage.input_digest,
		digest(
			&json!({"state":{"history":"bounded"},"questions":{"retain":"Keep?","discard":"Drop?"}})
		)
	);
	assert_eq!(input.boundary, json!({"step":7}));
}
#[rstest]
#[case("node")]
#[case("definition_digest")]
#[case("configuration_digest")]
#[case("kind")]
#[tokio::test]
async fn changed_approved_provider_never_reserves_or_calls(world: World, #[case] changed: &str) {
	{
		let mut state = world.0.lock().unwrap();
		match changed {
			"node" => {
				state.description["semantic"]["compactor"]["node_id"] = json!("aidash://other")
			}
			"definition_digest" => {
				state.description["semantic"]["compactor"]["digest"] = json!("wrong")
			}
			"configuration_digest" => {
				state.description["semantic"]["compactor"]["configuration_digest"] = json!("wrong")
			}
			"kind" => state.entry.kind = "model".into(),
			_ => panic!("case"),
		}
	}
	assert!(matches!(
		invoke(&world).await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	let state = world.0.lock().unwrap();
	assert!(state.input.is_none());
	assert_eq!(state.settled, 0);
	assert!(
		!state
			.events
			.iter()
			.any(|event| event == "provider.approved" || event == "provider.ask")
	);
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn missing_home_compactor_preserves_context_budget_failure(
	world: World,
	#[case] disabled: bool,
) {
	{
		let mut state = world.0.lock().unwrap();
		if disabled {
			state.description["semantic"] = json!({"mode":"disabled"});
		} else {
			state.description["semantic"]["compactor"] = Value::Null;
		}
	}
	assert!(matches!(
		invoke(&world).await,
		Err(Error::RemoteSemantic(Failure::ContextBudget))
	));
	assert_eq!(
		world.0.lock().unwrap().events,
		["suspend", "refresh", "description"]
	);
}
#[rstest]
#[case("approval")]
#[case("permission")]
#[tokio::test]
async fn inherited_and_current_authority_are_required_before_provider_work(
	world: World,
	#[case] denied: &str,
) {
	{
		let mut state = world.0.lock().unwrap();
		if denied == "approval" {
			state.approved = false;
		} else {
			state.denied = true;
		}
	}
	assert!(matches!(invoke(&world).await, Err(Error::Forbidden)));
	let state = world.0.lock().unwrap();
	assert!(state.input.is_none());
	assert!(
		!state
			.events
			.iter()
			.any(|event| event == "provider.approved")
	);
}
#[rstest]
#[case("factory", Failure::Configuration)]
#[case("credential", Failure::Configuration)]
#[case("request", Failure::ContextBudget)]
#[tokio::test]
async fn provider_preflight_failure_does_not_admit_an_allowance(
	world: World,
	#[case] failed: &str,
	#[case] expected: Failure,
) {
	{
		let mut state = world.0.lock().unwrap();
		match failed {
			"factory" => state.factory_fails = true,
			"credential" => state.credential_fails = true,
			"request" => state.oversized = true,
			_ => panic!("case"),
		}
	}
	assert!(matches!(invoke(&world).await,Err(Error::RemoteSemantic(actual)) if actual==expected));
	let state = world.0.lock().unwrap();
	assert!(state.input.is_none());
	assert!(!state.events.iter().any(|event| event == "admit"));
}
#[rstest]
#[tokio::test]
async fn provider_failure_still_settles_the_full_charge_and_refreshes_authority(world: World) {
	world.0.lock().unwrap().provider_fails = true;
	assert!(matches!(
		invoke(&world).await,
		Err(Error::RemoteSemantic(Failure::Unavailable))
	));
	let state = world.0.lock().unwrap();
	assert_eq!(state.settled, 1);
	assert!(state.held);
	assert_eq!(state.events.last().unwrap(), "refresh");
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn lost_worker_authority_preserves_the_existing_failure_boundary(
	world: World,
	#[case] after_call: bool,
) {
	world.0.lock().unwrap().available = if after_call {
		VecDeque::from([true, false])
	} else {
		VecDeque::from([false])
	};
	assert!(matches!(invoke(&world).await, Err(Error::Forbidden)));
	let state = world.0.lock().unwrap();
	assert!(!state.held);
	assert_eq!(state.settled, usize::from(after_call));
	assert_eq!(state.input.is_some(), after_call);
}
#[rstest]
#[case("suspend")]
#[case("refresh")]
#[case("description")]
#[case("catalog")]
#[case("compaction.invoke")]
#[case("admit")]
#[case("settle")]
#[tokio::test]
async fn boundary_errors_remain_identifiable_without_extra_retries(
	world: World,
	#[case] failed: &'static str,
) {
	world.0.lock().unwrap().failure = Some(failed);
	assert!(
		matches!(invoke(&world).await,Err(Error::External(message)) if message==format!("{failed} failed"))
	);
	let state = world.0.lock().unwrap();
	assert_eq!(state.settled, 0);
	assert_eq!(state.events.last().unwrap(), failed);
}
#[rstest]
#[case("admit", false)]
#[case("provider.ask", true)]
#[case("settle", true)]
#[tokio::test]
async fn cancellation_retains_only_the_already_admitted_allowance(
	world: World,
	#[case] paused: &'static str,
	#[case] admitted: bool,
) {
	world.0.lock().unwrap().pause = Some(paused);
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(10), invoke(&world))
			.await
			.is_err()
	);
	let state = world.0.lock().unwrap();
	assert!(!state.held);
	assert_eq!(state.input.is_some(), admitted);
	assert_eq!(state.settled, 0);
	assert_eq!(state.events.last().unwrap(), paused);
}

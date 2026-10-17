use super::*;
use crate::ports::catalog::CatalogScope;
use aidash_domain::{
	RunControl, Task, TaskStatus,
	context::Context,
	decision::DeciderConfig,
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
use serde_json::{Value, json};
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
	input: Option<Input>,
	settled: usize,
}
fn reference() -> EntityRef {
	EntityRef {
		id: "decider".into(),
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
	let entry: Entry =
		serde_json::from_value(json!({"id":"decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Decider"},"description":{},"config":crate::test_support::decider_config()}))
		.unwrap();
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
			binding_snapshot: crate::test_support::snapshot("aidash://receiver", "agent"),
			generation: None,
			lineage: vec![],
			node_id: "aidash://executor".into(),
			authority_digest: digest(&json!({})),
			agent: entry.clone(),
			definitions: vec![],
			semantic_memory: 1,
			decider: Some(reference()),
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
			decider: Some(Box::new(provider)),
			summarizer: None,
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
			decider: Some(provider),
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
		assert_eq!(action, "decision.invoke");
		self.0.point("decision.invoke").await?;
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
fn config() -> DeciderConfig {
	serde_json::from_value(crate::test_support::decider_config()).unwrap()
}
async fn resolve(world: &World) -> Result<Option<Entry>> {
	decider(&mut Authority(world.clone()), &world.run()).await
}
async fn admit_attempt(world: &World) -> Result<Input> {
	admit(
		&mut Authority(world.clone()),
		&world.run(),
		Uuid::from_u128(9),
		format!("sha256:{}", "a".repeat(64)),
		&config(),
	)
	.await
}
async fn settle_attempt(world: &World, input: &Input) -> Result<()> {
	settle(&mut Authority(world.clone()), &world.run(), input).await
}
#[rstest]
#[tokio::test]
async fn the_pinned_decider_is_resolved_under_a_fresh_lease_and_its_approval(world: World) {
	let entry = resolve(&world).await.unwrap().unwrap();
	assert_eq!((&*entry.id, &*entry.kind), ("decider", "decider"));
	let state = world.0.lock().unwrap();
	assert_eq!(
		state.events,
		[
			"suspend",
			"refresh",
			"description",
			"catalog",
			"decision.invoke"
		]
	);
	assert!(state.input.is_none());
}
#[rstest]
#[case("node")]
#[case("definition_digest")]
#[case("configuration_digest")]
#[case("kind")]
#[tokio::test]
async fn a_changed_pinned_decider_is_refused(world: World, #[case] changed: &str) {
	{
		let mut state = world.0.lock().unwrap();
		match changed {
			"node" => state.description["semantic"]["decider"]["node_id"] = json!("aidash://other"),
			"definition_digest" => {
				state.description["semantic"]["decider"]["digest"] = json!("wrong")
			}
			"configuration_digest" => {
				state.description["semantic"]["decider"]["configuration_digest"] = json!("wrong")
			}
			"kind" => state.entry.kind = "model".into(),
			_ => panic!("case"),
		}
	}
	assert!(matches!(
		resolve(&world).await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn a_home_without_a_decider_leaves_the_run_unpruned(world: World, #[case] disabled: bool) {
	{
		let mut state = world.0.lock().unwrap();
		if disabled {
			state.description["semantic"] = json!({"mode":"disabled"});
		} else {
			state.description["semantic"]["decider"] = Value::Null;
		}
	}
	assert!(resolve(&world).await.unwrap().is_none());
	assert_eq!(
		world.0.lock().unwrap().events,
		["suspend", "refresh", "description"]
	);
}
#[rstest]
#[case("approval")]
#[case("permission")]
#[tokio::test]
async fn inherited_and_current_authority_are_required(world: World, #[case] denied: &str) {
	{
		let mut state = world.0.lock().unwrap();
		if denied == "approval" {
			state.approved = false;
		} else {
			state.denied = true;
		}
	}
	assert!(matches!(resolve(&world).await, Err(Error::Forbidden)));
}
#[rstest]
#[tokio::test]
async fn lost_worker_authority_resolves_nothing(world: World) {
	world.0.lock().unwrap().available = VecDeque::from([false]);
	assert!(matches!(resolve(&world).await, Err(Error::Forbidden)));
	assert!(!world.0.lock().unwrap().held);
}
#[rstest]
#[tokio::test]
async fn an_attempt_is_admitted_at_the_largest_request_and_settled_after_its_result(world: World) {
	let input = admit_attempt(&world).await.unwrap();
	{
		let state = world.0.lock().unwrap();
		assert_eq!(state.events, ["suspend", "admit", "refresh"]);
		assert!(state.held);
		assert_eq!(input.usage.attempt_id, Uuid::from_u128(9));
		assert_eq!(input.usage.reserved_tokens, 600_000 + 1024);
		assert_eq!(
			input.usage.input_digest,
			format!("sha256:{}", "a".repeat(64))
		);
		assert_eq!(input.boundary, json!({"step":7}));
		// The reservation stays open while the attempt is dispatched.
		assert_eq!(state.settled, 0);
	}
	settle_attempt(&world, &input).await.unwrap();
	let state = world.0.lock().unwrap();
	assert_eq!(
		state.events,
		[
			"suspend", "admit", "refresh", "suspend", "settle", "refresh"
		]
	);
	assert_eq!(state.settled, 1);
	assert!(state.held);
}
#[rstest]
#[tokio::test]
async fn authority_lost_after_settling_keeps_the_charge(world: World) {
	world.0.lock().unwrap().available = VecDeque::from([true, false]);
	let input = admit_attempt(&world).await.unwrap();
	assert!(matches!(
		settle_attempt(&world, &input).await,
		Err(Error::Forbidden)
	));
	let state = world.0.lock().unwrap();
	assert_eq!(state.settled, 1);
	assert!(!state.held);
}
#[rstest]
#[case("suspend")]
#[case("admit")]
#[tokio::test]
async fn admission_errors_remain_identifiable_without_extra_retries(
	world: World,
	#[case] failed: &'static str,
) {
	world.0.lock().unwrap().failure = Some(failed);
	assert!(
		matches!(admit_attempt(&world).await,Err(Error::External(message)) if message==format!("{failed} failed"))
	);
	let state = world.0.lock().unwrap();
	assert_eq!(state.settled, 0);
	assert_eq!(state.events.last().unwrap(), failed);
}
#[rstest]
#[tokio::test]
async fn authority_lost_after_admission_keeps_the_open_charge(world: World) {
	world.0.lock().unwrap().available = VecDeque::from([false]);
	assert!(matches!(admit_attempt(&world).await, Err(Error::Forbidden)));
	let state = world.0.lock().unwrap();
	assert!(state.input.is_some());
	assert!(!state.held);
}
#[rstest]
#[tokio::test]
async fn a_failed_settlement_is_reported(world: World) {
	let input = admit_attempt(&world).await.unwrap();
	world.0.lock().unwrap().failure = Some("settle");
	assert!(
		matches!(settle_attempt(&world, &input).await,Err(Error::External(message)) if message=="settle failed")
	);
	assert_eq!(world.0.lock().unwrap().settled, 0);
}
#[rstest]
#[tokio::test]
async fn a_cancelled_admission_reserves_nothing(world: World) {
	world.0.lock().unwrap().pause = Some("admit");
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(10), admit_attempt(&world))
			.await
			.is_err()
	);
	let state = world.0.lock().unwrap();
	assert!(!state.held);
	assert!(state.input.is_none());
	assert_eq!(state.events.last().unwrap(), "admit");
}

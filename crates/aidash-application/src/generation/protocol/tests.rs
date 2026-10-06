use super::*;
use crate::ports::generation::{
	dispatch::{DispatchPreparation, DispatchVisibility},
	protocol::{GenerationProtocolLease, GenerationProtocolReservation},
};
use aidash_domain::{
	RunControl, Task, TaskStatus,
	context::Context,
	federation::execution::{Definition, Inspection},
	generation::{dispatch::Record, remote::Ancestor},
	registry::{EntityRef, Entry},
	run_state::{RecoveryState, RunState, StateVersion, ThinkingState},
	semantic::remote::{Boundary, Provider},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct World(Arc<Mutex<State>>);
struct State {
	description: Description,
	run: Run,
	input: Input,
	record: Option<Record>,
	binding: Option<Uuid>,
	ready: bool,
	peer_verified: bool,
	worker_available: bool,
	bad_receipts: bool,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	events: Vec<String>,
	active: usize,
	commits: usize,
	rollbacks: usize,
	denials: usize,
	charges: usize,
}
fn entry(id: &str, kind: &str, config: Value) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,
        "name":{"en":id},"description":{},"config":config}))
	.unwrap()
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn digest() -> String {
	format!("sha256:{}", "a".repeat(64))
}
fn owner(node: &str, id: u128) -> Ancestor {
	Ancestor {
		node_id: node.into(),
		tenant: "tenant".into(),
		request_id: Uuid::from_u128(id),
		policy_id: "policy".into(),
		policy_revision: 4,
		depth: 1,
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
	}
}
#[fixture]
fn world() -> World {
	let now = DateTime::<Utc>::from_timestamp(1000, 0).unwrap();
	let run = Run {
		id: Uuid::from_u128(101),
		task_id: Uuid::from_u128(102),
		workspace_id: Uuid::from_u128(103),
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
	let model = entry(
		"model",
		"model",
		json!({"provider":"openrouter","model_id":"fixture",
        "endpoint":"https://provider.example/v1","credential_env":null,"context_window":4096,
        "max_output_tokens":1024,"modalities":["text"],"cost":{}}),
	);
	let compactor = entry(
		"compactor",
		"compactor",
		json!({"provider":"typesafe",
        "endpoint":"https://compaction.example/v1","model":"fixture","credential_env":"FIXTURE_KEY",
        "max_request_bytes":2048,"max_questions":10,"max_response_bytes":1024}),
	);
	let provider = |node: &str, definition: &Entry| Provider {
		node_id: node.into(),
		entry: reference(&definition.id),
		digest: digest(),
		configuration_digest: aidash_domain::registry::rules::digest(&definition.config),
	};
	let embedding = Provider {
		node_id: run.home_node.clone(),
		entry: reference("embedding"),
		digest: digest(),
		configuration_digest: digest(),
	};
	let compactor_provider = provider("aidash://executor", &compactor);
	let description = Description {
		grant_id: Uuid::from_u128(104),
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
			authority_digest: digest(),
			agent: entry(
				"agent",
				"agent",
				json!({"model":reference("model"),"instructions":"Work"}),
			),
			definitions: [model.clone(), compactor]
				.into_iter()
				.map(|metadata| Definition {
					entry: reference(&metadata.id),
					kind: metadata.kind.clone(),
					digest: digest(),
					metadata,
				})
				.collect(),
			semantic_memory: 1,
			compactor: Some(reference("compactor")),
		},
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
		semantic: Binding::RequiredHome {
			native: None,
			home_lineage: vec![owner("aidash://home", 11)],
			execution_lineage: vec![owner("aidash://executor", 22)],
			version: 1,
			index_revision: 3,
			index_digest: digest(),
			embedding: Box::new(embedding),
			compactor: Some(Box::new(compactor_provider)),
		},
	};
	let input = Input {
		usage: Usage {
			operation_id: Uuid::from_u128(105),
			attempt_id: Uuid::from_u128(105),
			dispatcher_node: "aidash://executor".into(),
			grant_id: description.grant_id,
			admission_id: run.id,
			purpose: Purpose::Inference,
			provider: provider("aidash://executor", &model),
			input_digest: digest(),
			reserved_tokens: 5120,
		},
		boundary: json!({"step":run.step}),
	};
	let record = Record {
		usage: json!(input.usage),
		digest: input.usage.digest().unwrap(),
		peer_node: run.home_node.clone(),
		boundary: input.boundary.clone(),
		state: "PREPARING".into(),
		finalization: None,
		peer_finalized: false,
	};
	World(Arc::new(Mutex::new(State {
		description,
		run,
		binding: Some(input.usage.admission_id),
		input,
		record: Some(record),
		ready: true,
		peer_verified: true,
		worker_available: true,
		bad_receipts: false,
		failure: None,
		pause: None,
		events: vec![],
		active: 0,
		commits: 0,
		rollbacks: 0,
		denials: 0,
		charges: 0,
	})))
}
impl World {
	fn input(&self) -> Input {
		self.0.lock().unwrap().input.clone()
	}
	fn events(&self) -> Vec<String> {
		self.0.lock().unwrap().events.clone()
	}
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
	fn lease(&self) -> Box<Lease> {
		let description = {
			let mut state = self.0.lock().unwrap();
			state.active += 1;
			state.description.clone()
		};
		Box::new(Lease {
			world: self.clone(),
			description,
			reserved: false,
			finished: false,
		})
	}
	fn embedding_input(&self) -> Input {
		let state = self.0.lock().unwrap();
		let Binding::RequiredHome { embedding, .. } = &state.description.semantic else {
			panic!("fixture binding")
		};
		let operation = Operation {
			id: Uuid::from_u128(106),
			home_node: state.run.home_node.clone(),
			grant_id: state.description.grant_id,
			admission_id: state.run.id,
			boundary: Boundary {
				step: state.run.step,
				input_sequence: 0,
				task_revision: 1,
				inputs_digest: digest(),
			},
			inputs: vec![],
			query: "日本".into(),
			max_tokens: 128,
			metadata: json!({}),
		};
		let mut input = state.input.clone();
		input.usage.purpose = Purpose::Embedding;
		input.usage.operation_id = operation.id;
		input.usage.provider = (**embedding).clone();
		input.usage.dispatcher_node = state.run.home_node.clone();
		input.usage.input_digest = operation.digest().unwrap();
		input.usage.reserved_tokens = (operation.query.len() + 1024) as i64;
		input.boundary = json!(operation);
		input
	}
}
struct Lease {
	world: World,
	description: Description,
	reserved: bool,
	finished: bool,
}
impl Lease {
	async fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
		self.world.point("lease.finish").await?;
		let mut state = self.world.0.lock().unwrap();
		self.finished = true;
		if result.is_ok() {
			state.commits += 1;
			state.charges += usize::from(self.reserved);
		} else {
			state.rollbacks += 1;
			state.denials += usize::from(matches!(result, Err(Error::Forbidden)));
		}
		result
	}
}
impl Drop for Lease {
	fn drop(&mut self) {
		let mut state = self.world.0.lock().unwrap();
		state.active -= 1;
		if !self.finished {
			state.rollbacks += 1;
		}
	}
}
#[async_trait]
impl GenerationProtocolLease for Lease {
	fn description(&self) -> &Description {
		&self.description
	}
	async fn binding_admission(&mut self, _: Uuid) -> Result<Option<Uuid>> {
		self.world.point("binding").await?;
		Ok(self.world.0.lock().unwrap().binding)
	}
	async fn finish_verification(mut self: Box<Self>, result: Result<bool>) -> Result<bool> {
		self.finish(result).await
	}
}
#[async_trait]
impl GenerationProtocolReservation for Lease {
	async fn reserve(&mut self, usage: &Usage) -> Result<Vec<Reserved>> {
		self.world.point("local.reserve").await?;
		self.reserved = true;
		Ok(vec![Reserved {
			owner: owner("aidash://executor", 22),
			attempt_id: usage.attempt_id,
			digest: usage.digest()?,
		}])
	}
	async fn finish_reservations(
		mut self: Box<Self>,
		result: Result<Vec<Reserved>>,
	) -> Result<Vec<Reserved>> {
		self.finish(result).await
	}
}
#[async_trait]
impl GenerationProtocolAuthority for World {
	fn node_id(&self) -> &str {
		"aidash://executor"
	}
	async fn leaf(&self, _: &str, _: Uuid, _: Uuid) -> Result<Box<dyn GenerationProtocolLease>> {
		self.point("leaf").await?;
		Ok(self.lease())
	}
	async fn grant(&self, _: &str, _: Uuid) -> Result<Box<dyn GenerationProtocolLease>> {
		self.point("grant").await?;
		Ok(self.lease())
	}
	async fn worker(&self, run: &Run) -> Result<Option<Box<dyn GenerationProtocolReservation>>> {
		assert_eq!(run.id, self.0.lock().unwrap().run.id);
		self.point("worker").await?;
		let available = self.0.lock().unwrap().worker_available;
		Ok(available.then(|| self.lease() as Box<dyn GenerationProtocolReservation>))
	}
	async fn verify_semantic(&self, _: &Description, operation: &Operation) -> Result<()> {
		assert_eq!(operation.query, "日本");
		self.point("semantic.verify").await
	}
	async fn verify_peer(&self, _: &str, input: &Input) -> Result<bool> {
		assert_eq!(input.usage.purpose, Purpose::Inference);
		self.point("peer.verify").await?;
		Ok(self.0.lock().unwrap().peer_verified)
	}
	async fn reserve_peer(&self, home: &str, input: &Input) -> Result<Vec<Reserved>> {
		assert_eq!(home, "aidash://home");
		assert_eq!(self.0.lock().unwrap().active, 0);
		self.point("peer.reserve").await?;
		let bad = self.0.lock().unwrap().bad_receipts;
		Ok(vec![Reserved {
			owner: owner(home, 11),
			attempt_id: input.usage.attempt_id,
			digest: if bad {
				"wrong".into()
			} else {
				input.usage.digest()?
			},
		}])
	}
}
#[async_trait]
impl GenerationProtocolRepository for World {
	async fn description(&self, run: Uuid, home: &str) -> Result<Description> {
		self.point("description").await?;
		let state = self.0.lock().unwrap();
		assert_eq!(run, state.run.id);
		assert_eq!(home, state.run.home_node);
		Ok(state.description.clone())
	}
	async fn run(&self, admission: Uuid) -> Result<Run> {
		self.point("run").await?;
		let state = self.0.lock().unwrap();
		assert_eq!(admission, state.run.id);
		Ok(state.run.clone())
	}
	async fn semantic_ready(&self, admission: Uuid, grant: Uuid, step: i32) -> Result<bool> {
		self.point("semantic.ready").await?;
		let state = self.0.lock().unwrap();
		assert_eq!(
			(admission, grant, step),
			(state.run.id, state.description.grant_id, state.run.step)
		);
		Ok(state.ready)
	}
}
struct Preparation {
	world: World,
	record: Option<Record>,
}
#[async_trait]
impl DispatchPreparation for Preparation {
	async fn insert(&mut self, input: &Input, peer: &str, digest: &str) -> Result<()> {
		self.world.point("prepare.insert").await?;
		self.record = Some(Record {
			usage: json!(input.usage),
			digest: digest.into(),
			peer_node: peer.into(),
			boundary: input.boundary.clone(),
			state: "PREPARING".into(),
			finalization: None,
			peer_finalized: false,
		});
		Ok(())
	}
	async fn record(&mut self, _: Uuid) -> Result<Record> {
		Ok(self.record.clone().unwrap())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.world.point("prepare.commit").await?;
		self.world.0.lock().unwrap().record = self.record;
		Ok(())
	}
}
struct Visibility(World);
#[async_trait]
impl DispatchVisibility for Visibility {
	async fn terminal_record(&mut self, _: Uuid) -> Result<Option<Record>> {
		Ok(self.0.0.lock().unwrap().record.clone())
	}
	async fn abort_stale_preparations(&mut self) -> Result<()> {
		panic!("admission does not reconcile")
	}
	async fn pending(&mut self) -> Result<Vec<Uuid>> {
		panic!("admission does not reconcile")
	}
	async fn suspend(&mut self) -> Result<()> {
		self.0.point("visibility.suspend").await
	}
}
#[async_trait]
impl GenerationDispatchRepository for World {
	fn node_id(&self) -> &str {
		"aidash://executor"
	}
	async fn begin_preparation(&self) -> Result<Box<dyn DispatchPreparation>> {
		self.point("prepare.begin").await?;
		Ok(Box::new(Preparation {
			world: self.clone(),
			record: None,
		}))
	}
	async fn record(&self, _: Uuid) -> Result<Option<Record>> {
		self.point("dispatch.record").await?;
		Ok(self.0.lock().unwrap().record.clone())
	}
	async fn admit(&self, input: &Input, receipts: &[Reserved]) -> Result<u64> {
		self.point("dispatch.admit").await?;
		assert_eq!(self.0.lock().unwrap().active, 0);
		assert_eq!(receipts.len(), 2);
		assert_eq!(receipts[0].owner.node_id, "aidash://home");
		assert_eq!(receipts[1].owner.node_id, "aidash://executor");
		assert!(
			receipts
				.iter()
				.all(|r| r.digest == input.usage.digest().unwrap())
		);
		self.0.lock().unwrap().record.as_mut().unwrap().state = "DISPATCHED".into();
		Ok(1)
	}
	async fn finalize(&self, _: Uuid, from: &str, state: &str, result: &Value) -> Result<u64> {
		self.point("dispatch.finalize").await?;
		let mut data = self.0.lock().unwrap();
		let record = data.record.as_mut().unwrap();
		if record.state != from {
			return Ok(0);
		}
		record.state = state.into();
		record.finalization = Some(result.clone());
		Ok(1)
	}
	async fn begin_visibility(&self) -> Result<Box<dyn DispatchVisibility>> {
		self.point("visibility").await?;
		Ok(Box::new(Visibility(self.clone())))
	}
	async fn mark_peer_finalized(&self, _: Uuid) -> Result<()> {
		self.point("peer.finalized").await?;
		self.0
			.lock()
			.unwrap()
			.record
			.as_mut()
			.unwrap()
			.peer_finalized = true;
		Ok(())
	}
}
#[async_trait]
impl GenerationDispatchSettlement for World {
	async fn local(&self, _: &Usage, result: &Finalization) -> Result<()> {
		assert_eq!(result, &Finalization::Aborted {});
		self.point("local.abort").await
	}
	async fn peer(&self, _: &str, input: &FinalizeInput) -> Result<bool> {
		assert_eq!(input.result, Finalization::Aborted {});
		self.point("peer.abort").await?;
		Ok(true)
	}
}

#[rstest]
fn exact_inference_provider_pins_and_maximum_allowance_are_enforced(world: World) {
	let state = world.0.lock().unwrap();
	exact_provider(&state.description, &state.input.usage).unwrap();
}
#[rstest]
#[case("node")]
#[case("entry")]
#[case("definition_digest")]
#[case("configuration_digest")]
#[case("allowance")]
fn each_inference_binding_change_is_forbidden(world: World, #[case] changed: &str) {
	let state = world.0.lock().unwrap();
	let mut usage = state.input.usage.clone();
	match changed {
		"node" => usage.provider.node_id = "aidash://other".into(),
		"entry" => usage.provider.entry.version = "2.0.0".into(),
		"definition_digest" => usage.provider.digest = "wrong".into(),
		"configuration_digest" => usage.provider.configuration_digest = "wrong".into(),
		"allowance" => usage.reserved_tokens -= 1,
		_ => panic!("case"),
	}
	assert!(matches!(
		exact_provider(&state.description, &usage),
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case(Purpose::Embedding)]
#[case(Purpose::Compaction)]
fn semantic_provider_must_match_every_pin_and_dispatcher(world: World, #[case] purpose: Purpose) {
	let state = world.0.lock().unwrap();
	let Binding::RequiredHome {
		embedding,
		compactor,
		..
	} = &state.description.semantic
	else {
		panic!("binding")
	};
	let mut usage = state.input.usage.clone();
	usage.purpose = purpose;
	usage.provider = match purpose {
		Purpose::Embedding => (**embedding).clone(),
		_ => (**compactor.as_ref().unwrap()).clone(),
	};
	usage.dispatcher_node = usage.provider.node_id.clone();
	if purpose == Purpose::Compaction {
		usage.reserved_tokens = 3072;
	}
	exact_provider(&state.description, &usage).unwrap();
	usage.provider.digest = "wrong".into();
	assert!(matches!(
		exact_provider(&state.description, &usage),
		Err(Error::Forbidden)
	));
	usage.provider = match purpose {
		Purpose::Embedding => (**embedding).clone(),
		_ => (**compactor.as_ref().unwrap()).clone(),
	};
	usage.dispatcher_node = "aidash://other".into();
	assert!(matches!(
		exact_provider(&state.description, &usage),
		Err(Error::Forbidden)
	));
}
#[rstest]
#[tokio::test]
async fn inference_reservation_verifies_the_peer_before_any_local_charge(world: World) {
	let input = world.input();
	let receipts = reserve(&world, "aidash://executor", input).await.unwrap();
	assert_eq!(receipts.len(), 1);
	assert_eq!(
		world.events(),
		[
			"grant",
			"binding",
			"peer.verify",
			"local.reserve",
			"lease.finish"
		]
	);
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges, state.commits), (0, 1, 1));
}
#[rstest]
#[tokio::test]
async fn embedding_reservation_verifies_the_exact_utf8_body_without_a_callback(world: World) {
	let input = world.embedding_input();
	assert_eq!(input.usage.reserved_tokens, 1030);
	reserve(&world, "aidash://home", input).await.unwrap();
	assert_eq!(
		world.events(),
		["leaf", "semantic.verify", "local.reserve", "lease.finish"]
	);
}
#[rstest]
#[case("id")]
#[case("grant")]
#[case("admission")]
#[case("home")]
#[case("digest")]
#[case("allowance")]
#[tokio::test]
async fn embedding_body_mismatch_rolls_back_before_local_charge(world: World, #[case] field: &str) {
	let mut input = world.embedding_input();
	match field {
		"id" => input.boundary["id"] = json!(Uuid::from_u128(999)),
		"grant" => input.boundary["grant_id"] = json!(Uuid::from_u128(999)),
		"admission" => input.boundary["admission_id"] = json!(Uuid::from_u128(999)),
		"home" => input.boundary["home_node"] = json!("aidash://other"),
		"digest" => input.usage.input_digest = format!("sha256:{}", "b".repeat(64)),
		"allowance" => input.usage.reserved_tokens -= 1,
		_ => panic!("case"),
	}
	if field != "digest" {
		input.usage.input_digest = aidash_domain::registry::rules::digest(&input.boundary);
	}
	assert!(matches!(
		reserve(&world, "aidash://home", input).await,
		Err(Error::Forbidden)
	));
	assert_eq!(world.events(), ["leaf", "lease.finish"]);
	let state = world.0.lock().unwrap();
	assert_eq!((state.charges, state.rollbacks, state.denials), (0, 1, 1));
}
#[rstest]
#[case("binding")]
#[case("peer")]
#[tokio::test]
async fn rejected_binding_or_peer_never_charges(world: World, #[case] rejected: &str) {
	{
		let mut state = world.0.lock().unwrap();
		if rejected == "binding" {
			state.binding = None;
		} else {
			state.peer_verified = false;
		}
	}
	assert!(matches!(
		reserve(&world, "aidash://executor", world.input()).await,
		Err(Error::Forbidden)
	));
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges, state.denials), (0, 0, 1));
}
#[rstest]
#[case("grant")]
#[case("binding")]
#[case("peer.verify")]
#[case("local.reserve")]
#[case("lease.finish")]
#[tokio::test]
async fn reservation_failures_preserve_adapter_errors_and_no_charge(
	world: World,
	#[case] failed: &'static str,
) {
	world.0.lock().unwrap().failure = Some(failed);
	assert!(
		matches!(reserve(&world, "aidash://executor", world.input()).await,
        Err(Error::External(message)) if message == format!("{failed} failed"))
	);
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges), (0, 0));
}
#[rstest]
#[tokio::test]
async fn wrong_authenticated_dispatcher_is_rejected_before_authority_lookup(world: World) {
	assert!(matches!(
		reserve(&world, "aidash://other", world.input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(world.events(), Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn cancellation_rolls_back_the_owned_lease(world: World) {
	world.0.lock().unwrap().pause = Some("local.reserve");
	assert!(
		tokio::time::timeout(
			std::time::Duration::from_millis(10),
			reserve(&world, "aidash://executor", world.input())
		)
		.await
		.is_err()
	);
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges, state.rollbacks), (0, 0, 1));
}
#[rstest]
#[tokio::test]
async fn verification_holds_leaf_authority_through_exact_step_and_semantic_readiness(world: World) {
	assert!(
		verify(&world, &world, &world, "aidash://home", world.input())
			.await
			.unwrap()
	);
	assert_eq!(
		world.events(),
		[
			"leaf",
			"dispatch.record",
			"run",
			"semantic.ready",
			"lease.finish"
		]
	);
	assert_eq!(world.0.lock().unwrap().active, 0);
}
#[rstest]
#[case("peer")]
#[case("state")]
#[case("step")]
#[case("digest")]
#[tokio::test]
async fn changed_dispatch_binding_never_passes_verification(world: World, #[case] changed: &str) {
	{
		let mut state = world.0.lock().unwrap();
		match changed {
			"peer" => state.record.as_mut().unwrap().peer_node = "aidash://other".into(),
			"state" => state.record.as_mut().unwrap().state = "DISPATCHED".into(),
			"step" => state.run.step += 1,
			"digest" => state.record.as_mut().unwrap().digest = "wrong".into(),
			_ => panic!("case"),
		}
	}
	assert!(matches!(
		verify(&world, &world, &world, "aidash://home", world.input()).await,
		Err(Error::Forbidden)
	));
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.denials, state.charges), (0, 1, 0));
}
#[rstest]
#[tokio::test]
async fn missing_semantic_readiness_preserves_pending_and_rolls_back(world: World) {
	world.0.lock().unwrap().ready = false;
	assert!(matches!(
		verify(&world, &world, &world, "aidash://home", world.input()).await,
		Err(Error::RemoteSemantic(Failure::Pending))
	));
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.denials, state.rollbacks), (0, 0, 1));
}
#[rstest]
#[tokio::test]
async fn disabled_semantics_skip_only_the_ready_query(world: World) {
	world.0.lock().unwrap().description.semantic = Binding::Disabled {};
	assert!(
		verify(&world, &world, &world, "aidash://home", world.input())
			.await
			.unwrap()
	);
	assert_eq!(
		world.events(),
		["leaf", "dispatch.record", "run", "lease.finish"]
	);
}
async fn admit_fixture(world: &World) -> Result<Input> {
	let run = world.0.lock().unwrap().run.clone();
	let input = world.input();
	admit(
		world,
		world,
		world,
		world,
		Admission {
			run: &run,
			attempt: input.usage.attempt_id,
			purpose: input.usage.purpose,
			input_digest: input.usage.input_digest,
			amount: input.usage.reserved_tokens,
		},
	)
	.await
}
#[rstest]
#[tokio::test]
async fn admission_reacquires_authority_only_after_exact_home_receipts(world: World) {
	let expected = world.input();
	let input = admit_fixture(&world).await.unwrap();
	assert_eq!(json!(input), json!(expected));
	assert_eq!(
		world.events(),
		[
			"description",
			"prepare.begin",
			"prepare.insert",
			"prepare.commit",
			"peer.reserve",
			"worker",
			"local.reserve",
			"lease.finish",
			"dispatch.admit"
		]
	);
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges), (0, 1));
	assert_eq!(state.record.as_ref().unwrap().state, "DISPATCHED");
}
#[rstest]
#[case("receipts")]
#[case("worker")]
#[tokio::test]
async fn unproven_home_or_lost_worker_authority_durably_aborts_before_charge(
	world: World,
	#[case] rejected: &str,
) {
	{
		let mut state = world.0.lock().unwrap();
		if rejected == "receipts" {
			state.bad_receipts = true;
		} else {
			state.worker_available = false;
		}
	}
	let result = admit_fixture(&world).await;
	assert!(matches!(
		result,
		Err(Error::Forbidden) | Err(Error::RemoteSemantic(Failure::ProviderContract))
	));
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges), (0, 0));
	let record = state.record.as_ref().unwrap();
	assert_eq!(record.state, "ABORTED");
	assert!(record.peer_finalized);
	assert_eq!(record.finalization, Some(json!({"state":"aborted"})));
	if rejected == "receipts" {
		assert!(!state.events.iter().any(|event| event == "worker"));
	}
}
#[rstest]
#[case("peer.reserve")]
#[case("worker")]
#[case("local.reserve")]
#[case("lease.finish")]
#[case("dispatch.admit")]
#[tokio::test]
async fn admission_errors_preserve_the_failure_after_durable_abort(
	world: World,
	#[case] failed: &'static str,
) {
	world.0.lock().unwrap().failure = Some(failed);
	assert!(
		matches!(admit_fixture(&world).await, Err(Error::External(message)) if message == format!("{failed} failed"))
	);
	let state = world.0.lock().unwrap();
	assert_eq!(state.active, 0);
	assert_eq!(state.record.as_ref().unwrap().state, "ABORTED");
	assert_eq!(
		state.record.as_ref().unwrap().finalization,
		Some(json!({"state":"aborted"}))
	);
}
#[rstest]
#[tokio::test]
async fn cancellation_during_home_rpc_leaves_recoverable_preparation(world: World) {
	world.0.lock().unwrap().pause = Some("peer.reserve");
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(10), admit_fixture(&world))
			.await
			.is_err()
	);
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.charges), (0, 0));
	assert_eq!(state.record.as_ref().unwrap().state, "PREPARING");
	assert!(
		!state
			.events
			.iter()
			.any(|event| event == "worker" || event == "dispatch.finalize")
	);
}

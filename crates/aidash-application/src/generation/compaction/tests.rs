use super::*;
use crate::{
	generation::test_support,
	ports::{
		CompactionClassifier,
		catalog::CatalogScope,
		generation::{compaction::GenerationCompactionSession, publication::GenerationLive},
	},
};
use aidash_domain::{
	generation::{policy::Compaction, requests::Request},
	policy::Resource,
	registry::{CompactorConfig, EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Ledger {
	calls: BTreeMap<Uuid, usize>,
	attempts: Vec<(Uuid, Attempt)>,
}
struct State {
	jobs: Vec<Request>,
	specs: BTreeMap<Uuid, Value>,
	document: Option<Value>,
	approved: bool,
	enabled: bool,
	denied: Option<String>,
	exhausted: Option<Uuid>,
	fail: Option<String>,
	pause: Option<String>,
	ledger: Ledger,
	events: Vec<String>,
	active: usize,
	commits: usize,
}
#[derive(Clone)]
struct World(Arc<Mutex<State>>);
#[fixture]
fn world() -> World {
	let mut jobs = vec![test_support::request(11), test_support::request(22)];
	jobs[0].agent_id = "child".into();
	jobs[1].agent_id = "parent".into();
	for job in &mut jobs {
		job.expires_at = DateTime::from_timestamp(2000, 0).unwrap();
	}
	let mut spec: Spec = serde_json::from_value(test_support::specification()).unwrap();
	spec.compaction = Some(Compaction {
		provider: EntityRef {
			id: "compactor".into(),
			version: "1.0.0".into(),
		},
		calls_per_agent: 2,
		call_budget: 4,
	});
	World(Arc::new(Mutex::new(State {
		specs: jobs.iter().map(|job| (job.id, json!(spec))).collect(),
		ledger: Ledger {
			calls: jobs.iter().map(|job| (job.id, 0)).collect(),
			attempts: vec![],
		},
		jobs,
		document: Some(
			json!({"id":"compactor","version":"1.0.0","kind":"compactor","name":{"en":"Compactor"},"description":{"en":""},"config":{"provider":"typesafe","endpoint":"https://compaction.example/v1","model":"fixture","credential_env":"FIXTURE_COMPACTION_KEY","max_request_bytes":1024,"max_questions":10,"max_response_bytes":1024}}),
		),
		approved: true,
		enabled: true,
		denied: None,
		exhausted: None,
		fail: None,
		pause: None,
		events: vec![],
		active: 0,
		commits: 0,
	})))
}
async fn point(world: &World, name: String) -> Result<()> {
	let (fail, pause) = {
		let mut state = world.0.lock().unwrap();
		state.events.push(name.clone());
		(
			state.fail.as_ref() == Some(&name),
			state.pause.as_ref() == Some(&name),
		)
	};
	if fail {
		return Err(Error::External("fixture fault".into()));
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Authority(World);
#[async_trait]
impl GenerationCompactionAuthority for Authority {
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>> {
		assert_eq!(node, "aidash://origin");
		point(&self.0, "ancestors".into()).await?;
		let state = self.0.0.lock().unwrap();
		Ok(state
			.jobs
			.iter()
			.map(|job| (job.id, state.specs[&job.id].clone()))
			.collect())
	}
	fn live(&mut self) -> &mut dyn GenerationLive {
		self
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		self
	}
}
#[async_trait]
impl GenerationLive for Authority {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn now(&self) -> DateTime<Utc> {
		DateTime::from_timestamp(1500, 0).unwrap()
	}
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>> {
		assert_eq!(node, "aidash://origin");
		assert_eq!((&*agent.id, &*agent.version), ("child", "1.0.0"));
		point(&self.0, "live".into()).await?;
		Ok(self.0.0.lock().unwrap().jobs.clone())
	}
	async fn policy_enabled(&mut self, _: &Request) -> Result<bool> {
		Ok(self.0.0.lock().unwrap().enabled)
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
	fn approved(&self, reference: &EntityRef) -> bool {
		assert_eq!(
			(&*reference.id, &*reference.version),
			("compactor", "1.0.0")
		);
		self.0.0.lock().unwrap().approved
	}
	fn remember(&mut self, _: &EntityRef) {}
	async fn distribution_lock(&mut self) -> Result<()> {
		panic!("the caller retains its inherited lease")
	}
	async fn document(&mut self, _: &EntityRef) -> Result<Option<Value>> {
		point(&self.0, "document".into()).await?;
		Ok(self.0.0.lock().unwrap().document.clone())
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		panic!("compaction uses an exact provider")
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
		point(&self.0, action.into()).await?;
		if self.0.0.lock().unwrap().denied.as_deref() == Some(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		panic!("compaction requires exact authority")
	}
	async fn active(&mut self, _: &Entry) -> Result<bool> {
		panic!("compaction cannot discover providers")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("compaction uses its retained definition")
	}
}
struct Provider(World);
struct Transport(World);
impl GenerationCompactionProvider for Provider {
	fn approved(&self, config: CompactorConfig) -> Result<Arc<dyn ApprovedCompactionTransport>> {
		assert_eq!(
			(&*config.endpoint, &*config.model),
			("https://compaction.example/v1", "fixture")
		);
		self.0
			.0
			.lock()
			.unwrap()
			.events
			.push("provider.approved".into());
		Ok(Arc::new(Transport(self.0.clone())))
	}
}
impl ApprovedCompactionTransport for Transport {
	fn check_request(&self, state: &Value, questions: &CompactionQuestions) -> Result<usize> {
		assert_eq!(questions.len(), 2);
		self.0
			.0
			.lock()
			.unwrap()
			.events
			.push("provider.check".into());
		if state["oversized"] == true {
			return Err(Error::Invalid(
				"compaction request exceeds approved bounds".into(),
			));
		}
		Ok(130)
	}
}
#[async_trait]
impl CompactionClassifier for Transport {
	async fn ask(&self, state: &Value, questions: &CompactionQuestions) -> Result<Value> {
		assert_eq!(state, &json!({"history":"bounded"}));
		assert_eq!(questions.len(), 2);
		{
			let state = self.0.0.lock().unwrap();
			assert_eq!(state.active, 0);
			assert!(state.commits > 0);
		}
		point(&self.0, "provider.call".into()).await?;
		Ok(json!({"p1":0.9,"p2":0.1}))
	}
}
struct Repository(World);
struct Session {
	world: World,
	ledger: Ledger,
	committed: bool,
}
impl Drop for Session {
	fn drop(&mut self) {
		let mut state = self.world.0.lock().unwrap();
		state.active -= 1;
		state
			.events
			.push(if self.committed { "drop" } else { "rollback" }.into());
	}
}
#[async_trait]
impl GenerationCompactionRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://origin"
	}
	async fn begin(&self) -> Result<Box<dyn GenerationCompactionSession>> {
		point(&self.0, "begin".into()).await?;
		let mut state = self.0.0.lock().unwrap();
		state.active += 1;
		Ok(Box::new(Session {
			world: self.0.clone(),
			ledger: state.ledger.clone(),
			committed: false,
		}))
	}
}
#[async_trait]
impl GenerationCompactionSession for Session {
	async fn charge(&mut self, request: Uuid) -> Result<bool> {
		point(&self.world, format!("charge:{}", request.as_u128())).await?;
		if self.world.0.lock().unwrap().exhausted == Some(request) {
			return Ok(false);
		}
		*self.ledger.calls.get_mut(&request).unwrap() += 1;
		Ok(true)
	}
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()> {
		point(&self.world, format!("reserve:{}", request.as_u128())).await?;
		self.ledger.attempts.push((request, attempt.clone()));
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		point(&self.world, "commit".into()).await?;
		{
			let mut state = self.world.0.lock().unwrap();
			state.ledger = self.ledger.clone();
			state.commits += 1;
		}
		self.committed = true;
		Ok(())
	}
}
fn questions() -> CompactionQuestions {
	json!({"p1":"retain this call?","p2":"retain the complete result?"})
		.as_object()
		.unwrap()
		.clone()
}
fn context() -> Context {
	Context {
		run: Uuid::from_u128(77),
		task: Uuid::from_u128(2),
		agent: EntityRef {
			id: "child".into(),
			version: "1.0.0".into(),
		},
	}
}
async fn prepare(
	world: &World,
	state: &Value,
) -> Result<Option<Arc<dyn ApprovedCompactionTransport>>> {
	reserve(
		&mut Authority(world.clone()),
		&Repository(world.clone()),
		&Provider(world.clone()),
		&context(),
		state,
		&questions(),
	)
	.await
}
fn assert_ledger(world: &World, calls: usize, commits: usize) {
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.commits), (0, commits));
	assert_eq!(
		state.ledger.calls.values().copied().collect::<Vec<_>>(),
		[calls, calls]
	);
	assert_eq!(state.ledger.attempts.len(), 2 * calls);
}

#[rstest]
#[tokio::test]
async fn prepared_transport_is_returned_after_every_ancestor_is_durable(world: World) {
	let state = json!({"history":"bounded"});
	let transport = prepare(&world, &state).await.unwrap().unwrap();
	assert_ledger(&world, 1, 1);
	{
		let state = world.0.lock().unwrap();
		let first = &state.ledger.attempts[0].1;
		assert_eq!(first, &state.ledger.attempts[1].1);
		assert_eq!(
			(first.run, first.request_bytes, first.questions),
			(Uuid::from_u128(77), 130, 2)
		);
		assert_eq!(
			(&*first.provider.id, &*first.provider.version),
			("compactor", "1.0.0")
		);
		let begin = state
			.events
			.iter()
			.position(|event| event == "begin")
			.unwrap();
		for required in [
			"live",
			"registry.read",
			"compaction.invoke",
			"provider.check",
		] {
			assert!(
				state
					.events
					.iter()
					.position(|event| event == required)
					.unwrap() < begin
			);
		}
	}
	assert_eq!(
		transport.ask(&state, &questions()).await.unwrap(),
		json!({"p1":0.9,"p2":0.1})
	);
}

#[rstest]
#[tokio::test]
async fn ordinary_agents_keep_their_separate_transport_without_charges(world: World) {
	world.0.lock().unwrap().jobs.clear();
	assert!(prepare(&world, &json!({})).await.unwrap().is_none());
	assert_ledger(&world, 0, 0);
	assert_eq!(world.0.lock().unwrap().events, ["ancestors"]);
}

#[rstest]
#[case("disabled")]
#[case("expired")]
#[case("wrong_tenant")]
#[case("missing_approval")]
#[case("different_provider")]
#[case("inherited_unapproved")]
#[case("catalog_missing")]
#[case("registry.read")]
#[case("compaction.invoke")]
#[case("wrong_kind")]
#[case("oversized")]
#[tokio::test]
async fn denied_or_unbounded_requests_never_reserve_provider_calls(
	world: World,
	#[case] reason: &str,
) {
	{
		let mut state = world.0.lock().unwrap();
		match reason {
			"disabled" => state.enabled = false,
			"expired" => state.jobs[1].expires_at = DateTime::from_timestamp(1500, 0).unwrap(),
			"wrong_tenant" => state.jobs[1].tenant = "other".into(),
			"missing_approval" => state
				.specs
				.get_mut(&Uuid::from_u128(22))
				.unwrap()
				.as_object_mut()
				.unwrap()
				.remove("compaction")
				.map(|_| ())
				.unwrap(),
			"different_provider" => {
				state.specs.get_mut(&Uuid::from_u128(22)).unwrap()["compaction"]["provider"]["version"] =
					json!("2.0.0")
			}
			"inherited_unapproved" => state.approved = false,
			"catalog_missing" => state.document = None,
			"registry.read" | "compaction.invoke" => state.denied = Some(reason.into()),
			"wrong_kind" => state.document.as_mut().unwrap()["kind"] = json!("model"),
			"oversized" => {}
			_ => panic!("unknown denial fixture"),
		}
	}
	let state = if reason == "oversized" {
		json!({"oversized":true})
	} else {
		json!({})
	};
	let result = prepare(&world, &state).await;
	if reason == "missing_approval" {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="generated context requires a separately approved compaction provider")
		);
	} else if reason == "oversized" {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="compaction request exceeds approved bounds")
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_ledger(&world, 0, 0);
	assert!(
		!world
			.0
			.lock()
			.unwrap()
			.events
			.iter()
			.any(|event| event == "begin")
	);
}

#[rstest]
#[case("begin")]
#[case("charge:11")]
#[case("reserve:11")]
#[case("charge:22")]
#[case("reserve:22")]
#[case("commit")]
#[tokio::test]
async fn failed_reservations_roll_back_all_ancestors(world: World, #[case] fault: &str) {
	world.0.lock().unwrap().fail = Some(fault.into());
	assert!(matches!(
		prepare(&world, &json!({})).await,
		Err(Error::External(_))
	));
	assert_ledger(&world, 0, 0);
}

#[rstest]
#[case(11)]
#[case(22)]
#[tokio::test]
async fn exhausted_call_slots_cannot_partially_charge_the_lineage(
	world: World,
	#[case] request: u128,
) {
	world.0.lock().unwrap().exhausted = Some(Uuid::from_u128(request));
	assert!(
		matches!(prepare(&world,&json!({})).await,Err(Error::Invalid(message)) if message=="generated compaction call budget exhausted")
	);
	assert_ledger(&world, 0, 0);
}

#[rstest]
#[case("charge:22")]
#[case("reserve:22")]
#[tokio::test]
async fn cancelled_reservation_rolls_back_every_call_slot(world: World, #[case] pause: &str) {
	world.0.lock().unwrap().pause = Some(pause.into());
	let state = json!({});
	{
		let mut future = Box::pin(prepare(&world, &state));
		assert!(futures_util::poll!(&mut future).is_pending());
		assert_eq!(world.0.lock().unwrap().active, 1);
	}
	assert_ledger(&world, 0, 0);
	assert_eq!(world.0.lock().unwrap().events.last().unwrap(), "rollback");
}

#[rstest]
#[tokio::test]
async fn failed_provider_calls_stay_charged_and_retries_get_fresh_attempts(world: World) {
	let state = json!({"history":"bounded"});
	let first = prepare(&world, &state).await.unwrap().unwrap();
	world.0.lock().unwrap().fail = Some("provider.call".into());
	assert!(matches!(
		first.ask(&state, &questions()).await,
		Err(Error::External(_))
	));
	assert_ledger(&world, 1, 1);
	world.0.lock().unwrap().fail = None;
	let second = prepare(&world, &state).await.unwrap().unwrap();
	assert_eq!(
		second.ask(&state, &questions()).await.unwrap(),
		json!({"p1":0.9,"p2":0.1})
	);
	assert_ledger(&world, 2, 2);
	let state = world.0.lock().unwrap();
	assert_ne!(state.ledger.attempts[0].1.id, state.ledger.attempts[2].1.id);
	assert_eq!(
		state
			.events
			.iter()
			.filter(|event| *event == "provider.call")
			.count(),
		2
	);
}

#[rstest]
#[tokio::test]
async fn cancelled_provider_calls_retain_the_durable_call_charge(world: World) {
	let state = json!({"history":"bounded"});
	let transport = prepare(&world, &state).await.unwrap().unwrap();
	world.0.lock().unwrap().pause = Some("provider.call".into());
	let questions = questions();
	{
		let mut future = Box::pin(transport.ask(&state, &questions));
		assert!(futures_util::poll!(&mut future).is_pending());
	}
	assert_ledger(&world, 1, 1);
}

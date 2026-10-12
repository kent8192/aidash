use super::*;
use crate::{
	generation::test_support,
	ports::{
		catalog::CatalogScope,
		generation::{publication::GenerationLive, summary::GenerationSummarySession},
	},
};
use aidash_domain::{
	generation::{policy::Summary, requests::Request},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

struct State {
	jobs: Vec<Request>,
	specs: BTreeMap<Uuid, Value>,
	document: Value,
	denied: Option<String>,
	exhausted: Option<Uuid>,
	calls: BTreeMap<Uuid, usize>,
	attempts: Vec<(Uuid, Attempt)>,
	events: Vec<String>,
}
#[derive(Clone)]
struct World(Arc<Mutex<State>>);

fn model() -> EntityRef {
	EntityRef {
		id: "summarizer".into(),
		version: "1.0.0".into(),
	}
}
fn document() -> Value {
	json!({"id":"summarizer","version":"1.0.0","kind":"model","name":{"en":"Summarizer"},"description":{"en":""},"config":{"model":"fixture-summarizer"}})
}
fn approval(provider: EntityRef) -> Summary {
	Summary {
		provider,
		calls_per_agent: 2,
		call_budget: 4,
	}
}
#[fixture]
fn world() -> World {
	let mut jobs = vec![test_support::request(11), test_support::request(22)];
	jobs[0].agent_id = "child".into();
	jobs[1].agent_id = "parent".into();
	for job in &mut jobs {
		job.expires_at = DateTime::from_timestamp(2000, 0).unwrap();
	}
	let mut spec: Spec = serde_json::from_value(test_support::specification()).unwrap();
	spec.summary = Some(approval(model()));
	World(Arc::new(Mutex::new(State {
		specs: jobs.iter().map(|job| (job.id, json!(spec))).collect(),
		calls: jobs.iter().map(|job| (job.id, 0)).collect(),
		jobs,
		document: document(),
		denied: None,
		exhausted: None,
		attempts: vec![],
		events: vec![],
	})))
}
fn summarizer() -> SummaryProvider {
	let entry: Entry = serde_json::from_value(document()).unwrap();
	SummaryProvider {
		model: model(),
		definition_digest: digest(&serde_json::to_value(&entry).unwrap()),
	}
}
fn context() -> Context {
	Context {
		run: Uuid::from_u128(7),
		task: Uuid::from_u128(2),
		agent: EntityRef {
			id: "child".into(),
			version: "1.0.0".into(),
		},
	}
}

struct Authority(World);
#[async_trait]
impl GenerationCompactionAuthority for Authority {
	async fn ancestors(&mut self, node: &str) -> Result<Vec<(Uuid, Value)>> {
		assert_eq!(node, "aidash://origin");
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
	async fn jobs(&mut self, _: &str, _: &EntityRef) -> Result<Vec<Request>> {
		Ok(self.0.0.lock().unwrap().jobs.clone())
	}
	async fn policy_enabled(&mut self, _: &Request) -> Result<bool> {
		Ok(true)
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
			reference,
			&model(),
			"only the pinned summarizer is resolved"
		);
		true
	}
	fn remember(&mut self, _: &EntityRef) {}
	async fn distribution_lock(&mut self) -> Result<()> {
		panic!("the caller retains its inherited lease")
	}
	async fn document(&mut self, _: &EntityRef) -> Result<Option<Value>> {
		Ok(Some(self.0.0.lock().unwrap().document.clone()))
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		panic!("the summarizer is exact")
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
		let mut state = self.0.0.lock().unwrap();
		state.events.push(action.into());
		if state.denied.as_deref() == Some(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		panic!("summary requires exact authority")
	}
	async fn active(&mut self, _: &Entry) -> Result<bool> {
		panic!("summary cannot discover providers")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("summary uses its retained definition")
	}
}
struct Repository(World);
struct Session {
	world: World,
	calls: BTreeMap<Uuid, usize>,
	attempts: Vec<(Uuid, Attempt)>,
}
#[async_trait]
impl GenerationSummaryRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://origin"
	}
	async fn begin(&self) -> Result<Box<dyn GenerationSummarySession>> {
		let state = self.0.0.lock().unwrap();
		Ok(Box::new(Session {
			world: self.0.clone(),
			calls: state.calls.clone(),
			attempts: state.attempts.clone(),
		}))
	}
}
#[async_trait]
impl GenerationSummarySession for Session {
	async fn charge(&mut self, request: Uuid) -> Result<bool> {
		if self.world.0.lock().unwrap().exhausted == Some(request) {
			return Ok(false);
		}
		*self.calls.get_mut(&request).unwrap() += 1;
		Ok(true)
	}
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()> {
		self.attempts.push((request, attempt.clone()));
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		let mut state = self.world.0.lock().unwrap();
		state.calls = self.calls;
		state.attempts = self.attempts;
		state.events.push("commit".into());
		Ok(())
	}
}
async fn run(world: &World, summarizer: &SummaryProvider) -> Result<bool> {
	reserve(
		&mut Authority(world.clone()),
		&Repository(world.clone()),
		&context(),
		summarizer,
		512,
	)
	.await
}
fn unavailable(result: Result<bool>) -> bool {
	matches!(result, Err(Error::Context(Failure::SummaryUnavailable)))
}

#[rstest]
#[tokio::test]
async fn every_ancestor_is_charged_and_recorded_before_io(world: World) {
	assert!(run(&world, &summarizer()).await.unwrap());
	{
		let state = world.0.lock().unwrap();
		assert!(state.calls.values().all(|calls| *calls == 1));
		assert_eq!(state.attempts.len(), 2);
		let (first, second) = (&state.attempts[0].1, &state.attempts[1].1);
		assert_eq!(
			first, second,
			"one attempt identity is charged per ancestor"
		);
		assert_eq!(first.provider, model());
		assert_eq!(first.request_bytes, 512);
		assert_eq!(
			state.attempts.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
			state.jobs.iter().map(|job| job.id).collect::<Vec<_>>()
		);
		assert_eq!(
			state
				.events
				.iter()
				.filter(|e| e.as_str() == "model.infer")
				.count(),
			1
		);
	}
	// Every retry, including after a failed provider call, is charged again.
	assert!(run(&world, &summarizer()).await.unwrap());
	assert!(
		world
			.0
			.lock()
			.unwrap()
			.calls
			.values()
			.all(|calls| *calls == 2)
	);
}

#[rstest]
#[tokio::test]
async fn ordinary_runs_have_no_generated_charge(world: World) {
	world.0.lock().unwrap().jobs.clear();
	assert!(!run(&world, &summarizer()).await.unwrap());
	assert!(world.0.lock().unwrap().attempts.is_empty());
}

#[rstest]
#[tokio::test]
async fn recheck_repeats_ancestor_approval_without_charging(world: World) {
	let recheck_with = |world: &World| {
		let world = world.clone();
		async move {
			recheck(
				&mut Authority(world),
				"aidash://origin",
				&context(),
				&summarizer(),
			)
			.await
		}
	};
	recheck_with(&world).await.unwrap();
	{
		let mut state = world.0.lock().unwrap();
		assert!(state.calls.values().all(|calls| *calls == 0));
		assert!(state.attempts.is_empty());
		assert!(!state.events.contains(&"commit".into()));
		let parent = state.jobs[1].id;
		let mut spec: Spec = serde_json::from_value(state.specs[&parent].clone()).unwrap();
		spec.summary = None;
		state.specs.insert(parent, json!(spec));
	}
	assert!(matches!(
		recheck_with(&world).await,
		Err(Error::Context(Failure::SummaryUnavailable))
	));
}

#[rstest]
#[case("missing_child")]
#[case("missing_parent")]
#[case("different_parent")]
#[case("different_version")]
#[case("exhausted_parent")]
#[case("digest")]
#[case("kind")]
#[case("model.infer")]
#[case("registry.read")]
#[tokio::test]
async fn a_summarizer_not_approved_by_every_ancestor_is_refused_without_charges(
	world: World,
	#[case] reason: &str,
) {
	let mut summarizer = summarizer();
	{
		let mut state = world.0.lock().unwrap();
		let (child, parent) = (state.jobs[0].id, state.jobs[1].id);
		let mut set = |id: Uuid, approval: Option<Summary>| {
			let mut spec: Spec = serde_json::from_value(state.specs[&id].clone()).unwrap();
			spec.summary = approval;
			state.specs.insert(id, json!(spec));
		};
		match reason {
			"missing_child" => set(child, None),
			"missing_parent" => set(parent, None),
			"different_parent" => set(
				parent,
				Some(approval(EntityRef {
					id: "other".into(),
					version: "1.0.0".into(),
				})),
			),
			"different_version" => set(
				parent,
				Some(approval(EntityRef {
					id: "summarizer".into(),
					version: "2.0.0".into(),
				})),
			),
			"exhausted_parent" => state.exhausted = Some(parent),
			"digest" => summarizer.definition_digest = "sha256:other".into(),
			"kind" => state.document["kind"] = json!("compactor"),
			action => state.denied = Some(action.into()),
		}
	}
	let result = run(&world, &summarizer).await;
	if matches!(reason, "model.infer" | "registry.read") {
		// The execution boundary classifies catalog denials as SummaryUnavailable.
		assert!(matches!(result, Err(Error::Forbidden)));
	} else if reason == "kind" {
		assert!(result.is_err());
	} else {
		assert!(unavailable(result), "{reason}");
	}
	let state = world.0.lock().unwrap();
	assert!(state.calls.values().all(|calls| *calls == 0), "{reason}");
	assert!(state.attempts.is_empty(), "{reason}");
	assert!(!state.events.contains(&"commit".into()), "{reason}");
}

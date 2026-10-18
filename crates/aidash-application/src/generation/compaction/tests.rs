use super::*;
use crate::{
	generation::test_support,
	ports::{
		catalog::CatalogScope,
		generation::{compaction::GenerationCompactionSession, publication::GenerationLive},
	},
};
use aidash_domain::{
	generation::{policy::Compaction, requests::Request},
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
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
			id: "decider".into(),
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
			json!({"id":"decider","version":"1.0.0","kind":"decider","name":{"en":"Decider"},"description":{"en":""},"config":crate::test_support::decider_config()}),
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
		assert_eq!((&*reference.id, &*reference.version), ("decider", "1.0.0"));
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
		panic!("compaction uses an exact Decider")
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
		panic!("compaction cannot discover Deciders")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("compaction uses its retained definition")
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
	async fn reserved(&mut self, request: Uuid, attempt: Uuid) -> Result<bool> {
		Ok(self
			.ledger
			.attempts
			.iter()
			.any(|(id, saved)| *id == request && saved.id == attempt))
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
fn reference() -> EntityRef {
	EntityRef {
		id: "decider".into(),
		version: "1.0.0".into(),
	}
}
async fn resolve(world: &World) -> Result<Chain> {
	decider(&mut Authority(world.clone()), "aidash://origin", &context()).await
}
async fn charge_attempt(world: &World, attempt: u128) -> Result<BTreeMap<String, Uuid>> {
	charge(
		&mut Authority(world.clone()),
		&Repository(world.clone()),
		&context(),
		&reference(),
		Uuid::from_u128(attempt),
		600_000,
		2,
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
async fn the_agreed_decider_is_resolved_under_live_authority_and_its_approval(world: World) {
	let Chain::Generated(Some(entry)) = resolve(&world).await.unwrap() else {
		panic!("expected the agreed Decider");
	};
	assert_eq!((&*entry.id, &*entry.kind), ("decider", "decider"));
	let state = world.0.lock().unwrap();
	for required in ["live", "registry.read", "decision.invoke"] {
		assert!(
			state.events.iter().any(|event| event == required),
			"{required}"
		);
	}
	assert_eq!(state.commits, 0);
}

#[rstest]
#[tokio::test]
async fn every_ancestor_is_charged_with_the_attempt_before_dispatch(world: World) {
	let receipts = charge_attempt(&world, 5).await.unwrap();
	assert_ledger(&world, 1, 1);
	assert_eq!(
		receipts.keys().collect::<Vec<_>>(),
		[
			&format!("aidash://origin/generation/{}", Uuid::from_u128(11)),
			&format!("aidash://origin/generation/{}", Uuid::from_u128(22))
		]
	);
	assert!(
		receipts
			.values()
			.all(|receipt| *receipt == Uuid::from_u128(5))
	);
	let state = world.0.lock().unwrap();
	let first = &state.ledger.attempts[0].1;
	assert_eq!(first, &state.ledger.attempts[1].1);
	assert_eq!(
		(first.id, first.run, first.request_bytes, first.questions),
		(Uuid::from_u128(5), Uuid::from_u128(77), 600_000, 2)
	);
	assert_eq!(first.provider, reference());
	let begin = state
		.events
		.iter()
		.position(|event| event == "begin")
		.unwrap();
	let live = state
		.events
		.iter()
		.position(|event| event == "live")
		.unwrap();
	assert!(live < begin);
}

#[rstest]
#[tokio::test]
async fn ordinary_agents_follow_their_own_binding_without_charges(world: World) {
	world.0.lock().unwrap().jobs.clear();
	assert!(matches!(resolve(&world).await.unwrap(), Chain::Ordinary));
	assert_ledger(&world, 0, 0);
	assert_eq!(world.0.lock().unwrap().events, ["ancestors"]);
}

#[rstest]
#[case("missing_approval")]
#[case("different_provider")]
#[tokio::test]
async fn ancestors_that_disagree_leave_the_run_unpruned(world: World, #[case] reason: &str) {
	{
		let mut state = world.0.lock().unwrap();
		let spec = state.specs.get_mut(&Uuid::from_u128(22)).unwrap();
		if reason == "missing_approval" {
			spec.as_object_mut().unwrap().remove("compaction");
		} else {
			spec["compaction"]["provider"]["version"] = json!("2.0.0");
		}
	}
	assert!(matches!(
		resolve(&world).await.unwrap(),
		Chain::Generated(None)
	));
	// An ancestor that withdrew its approval is never charged.
	assert!(matches!(
		charge_attempt(&world, 5).await,
		Err(Error::Context(Failure::DeciderUnapproved))
	));
	assert_ledger(&world, 0, 0);
}

#[rstest]
#[case("disabled")]
#[case("expired")]
#[case("wrong_tenant")]
#[case("inherited_unapproved")]
#[case("catalog_missing")]
#[case("registry.read")]
#[case("decision.invoke")]
#[case("wrong_kind")]
#[tokio::test]
async fn denied_deciders_are_never_resolved(world: World, #[case] reason: &str) {
	{
		let mut state = world.0.lock().unwrap();
		match reason {
			"disabled" => state.enabled = false,
			"expired" => state.jobs[1].expires_at = DateTime::from_timestamp(1500, 0).unwrap(),
			"wrong_tenant" => state.jobs[1].tenant = "other".into(),
			"inherited_unapproved" => state.approved = false,
			"catalog_missing" => state.document = None,
			"registry.read" | "decision.invoke" => state.denied = Some(reason.into()),
			"wrong_kind" => state.document.as_mut().unwrap()["kind"] = json!("model"),
			_ => panic!("unknown denial fixture"),
		}
	}
	assert!(matches!(resolve(&world).await, Err(Error::Forbidden)));
	assert_ledger(&world, 0, 0);
}

#[rstest]
#[case("disabled")]
#[case("expired")]
#[tokio::test]
async fn a_chain_that_is_no_longer_live_is_never_charged(world: World, #[case] reason: &str) {
	{
		let mut state = world.0.lock().unwrap();
		if reason == "disabled" {
			state.enabled = false;
		} else {
			state.jobs[1].expires_at = DateTime::from_timestamp(1500, 0).unwrap();
		}
	}
	assert!(matches!(
		charge_attempt(&world, 5).await,
		Err(Error::Forbidden)
	));
	assert_ledger(&world, 0, 0);
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
		charge_attempt(&world, 5).await,
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
	assert!(matches!(
		charge_attempt(&world, 5).await,
		Err(Error::Context(Failure::DeciderCallsExhausted))
	));
	assert_ledger(&world, 0, 0);
}

#[rstest]
#[case("charge:22")]
#[case("reserve:22")]
#[tokio::test]
async fn cancelled_reservation_rolls_back_every_call_slot(world: World, #[case] pause: &str) {
	world.0.lock().unwrap().pause = Some(pause.into());
	{
		let mut future = Box::pin(charge_attempt(&world, 5));
		assert!(futures_util::poll!(&mut future).is_pending());
		assert_eq!(world.0.lock().unwrap().active, 1);
	}
	assert_ledger(&world, 0, 0);
	assert_eq!(world.0.lock().unwrap().events.last().unwrap(), "rollback");
}

#[rstest]
#[tokio::test]
async fn every_attempt_is_charged_once_for_every_ancestor(world: World) {
	charge_attempt(&world, 5).await.unwrap();
	charge_attempt(&world, 6).await.unwrap();
	assert_ledger(&world, 2, 2);
	let state = world.0.lock().unwrap();
	assert_ne!(state.ledger.attempts[0].1.id, state.ledger.attempts[2].1.id);
}
#[rstest]
#[tokio::test]
async fn a_retried_attempt_reuses_its_committed_charge(world: World) {
	// The charge committed, but recording the Decision Attempt failed, so the
	// same attempt is charged again.
	let first = charge_attempt(&world, 5).await.unwrap();
	let retried = charge_attempt(&world, 5).await.unwrap();
	assert_eq!(first, retried);
	// One call per ancestor, though both sessions committed.
	assert_ledger(&world, 1, 2);
}

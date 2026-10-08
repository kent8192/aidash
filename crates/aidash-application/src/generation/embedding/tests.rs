use super::*;
use crate::{
	generation::test_support,
	ports::{
		catalog::CatalogScope,
		generation::{embedding::GenerationEmbeddingSession, publication::GenerationLive},
	},
};
use aidash_domain::{
	generation::{
		policy::{Embedding, Spec},
		requests::Request,
	},
	policy::Resource,
	registry::Entry,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Ledger {
	balances: BTreeMap<Uuid, i64>,
	attempts: BTreeMap<Uuid, Attempt>,
	reports: BTreeMap<Uuid, Option<i64>>,
}
struct State {
	jobs: Vec<Request>,
	policies: BTreeMap<Uuid, Spec>,
	enabled: BTreeMap<Uuid, bool>,
	document: Option<Value>,
	approved: bool,
	denied: Option<String>,
	ledger: Ledger,
	calls: Vec<String>,
	fail: Option<String>,
	pause: Option<String>,
	active: usize,
	commits: usize,
}
#[derive(Clone)]
struct World(Arc<Mutex<State>>);
fn config() -> EmbeddingConfig {
	EmbeddingConfig {
		provider: "openai".into(),
		endpoint: "https://embedding.example/v1".into(),
		credential_env: Some("FIXTURE_EMBEDDING_KEY".into()),
		model: "fixture".into(),
		model_version: "1".into(),
		dimensions: 4,
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
	let reference = EntityRef {
		id: "embedding".into(),
		version: "1.0.0".into(),
	};
	let mut spec: Spec = serde_json::from_value(test_support::specification()).unwrap();
	spec.embedding = Some(Embedding {
		provider: reference,
		calls_per_agent: 2,
		call_budget: 4,
	});
	World(Arc::new(Mutex::new(State {
		policies: jobs.iter().map(|job| (job.id, spec.clone())).collect(),
		ledger: Ledger {
			balances: jobs.iter().map(|job| (job.id, 0)).collect(),
			attempts: BTreeMap::new(),
			reports: BTreeMap::new(),
		},
		jobs,
		enabled: BTreeMap::new(),
		document: Some(
			json!({"id":"embedding","version":"1.0.0","kind":"embedding","name":{"en":"Embedding"},"description":{"en":""},"config":config()}),
		),
		approved: true,
		denied: None,
		calls: vec![],
		fail: None,
		pause: None,
		active: 0,
		commits: 0,
	})))
}
async fn point(world: &World, name: String) -> Result<()> {
	let (fail, pause) = {
		let mut state = world.0.lock().unwrap();
		state.calls.push(name.clone());
		(
			state.fail.as_ref() == Some(&name),
			state.pause.as_ref() == Some(&name),
		)
	};
	if fail {
		return Err(Error::External("fixture database fault".into()));
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
struct Authority(World);

#[rstest]
#[case("success")]
#[case("late_charge")]
#[case("expired_origin")]
#[tokio::test]
async fn overlapping_origins_validate_every_lease_and_charge_one_ancestor_union(
	world: World,
	#[case] failure: &str,
) {
	let secondary = World(Arc::new(Mutex::new(State {
		jobs: vec![],
		policies: BTreeMap::new(),
		enabled: BTreeMap::new(),
		document: None,
		approved: true,
		denied: None,
		ledger: Ledger {
			balances: BTreeMap::new(),
			attempts: BTreeMap::new(),
			reports: BTreeMap::new(),
		},
		calls: vec![],
		fail: None,
		pause: None,
		active: 0,
		commits: 0,
	})));
	let last = Uuid::from_u128(33);
	{
		let mut primary = world.0.lock().unwrap();
		let mut other = secondary.0.lock().unwrap();
		other.jobs = primary.jobs.clone();
		other.jobs[1].id = last;
		other.policies = primary.policies.clone();
		other
			.policies
			.insert(last, primary.policies[&Uuid::from_u128(22)].clone());
		other.document = primary.document.clone();
		primary.ledger.balances.insert(last, 0);
		if failure == "late_charge" {
			primary.fail = Some("charge:33".into());
		}
		if failure == "expired_origin" {
			other.jobs[1].expires_at = DateTime::from_timestamp(1000, 0).unwrap();
		}
	}
	let mut a = Authority(world.clone());
	let mut b = Authority(secondary.clone());
	let result = reserve_many(
		&mut [&mut a, &mut b],
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(3),
		&config(),
		"日本",
		Origin::Index(Uuid::from_u128(77)),
	)
	.await;
	if failure != "success" {
		assert!(result.is_err());
		let state = world.0.lock().unwrap();
		assert_eq!((state.active, state.commits), (0, 0));
		assert!(state.ledger.balances.values().all(|amount| *amount == 0));
		assert!(state.ledger.attempts.is_empty());
		drop(state);
		if failure == "expired_origin" {
			assert!(!world.0.lock().unwrap().calls.contains(&"begin".into()));
		}
	} else {
		let receipt = result.unwrap().unwrap();
		assert_eq!(
			world
				.0
				.lock()
				.unwrap()
				.ledger
				.balances
				.values()
				.copied()
				.collect::<Vec<_>>(),
			vec![1030; 3]
		);
		receipt.settle(Some(2)).await.unwrap();
		let state = world.0.lock().unwrap();
		assert_eq!(
			state.ledger.balances.values().copied().collect::<Vec<_>>(),
			vec![2; 3]
		);
		assert_eq!(state.ledger.attempts.len(), 3);
		assert_eq!(
			state
				.calls
				.iter()
				.filter(|call| call.as_str() == "charge:11")
				.count(),
			1
		);
	}
}
#[async_trait]
impl GenerationEmbeddingAuthority for Authority {
	async fn requests(&mut self, node: &str) -> Result<Vec<Request>> {
		assert_eq!(node, "aidash://origin");
		point(&self.0, "requests".into()).await?;
		Ok(self.0.0.lock().unwrap().jobs.clone())
	}
	async fn pinned_policy(&mut self, job: &Request) -> Result<Spec> {
		assert_eq!(job.policy_revision, 7);
		point(&self.0, format!("pinned:{}", job.id.as_u128())).await?;
		Ok(self.0.0.lock().unwrap().policies[&job.id].clone())
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
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		point(&self.0, format!("current:{}", job.id.as_u128())).await?;
		Ok(*self
			.0
			.0
			.lock()
			.unwrap()
			.enabled
			.get(&job.id)
			.unwrap_or(&true))
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
			("embedding", "1.0.0")
		);
		self.0.0.lock().unwrap().approved
	}
	fn remember(&mut self, _: &EntityRef) {
		self.0.0.lock().unwrap().calls.push("remember".into());
	}
	async fn distribution_lock(&mut self) -> Result<()> {
		panic!("inherited authority retains its distribution lease")
	}
	async fn document(&mut self, _: &EntityRef) -> Result<Option<Value>> {
		point(&self.0, "catalog.document".into()).await?;
		Ok(self.0.0.lock().unwrap().document.clone())
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		panic!("embedding must use its exact approved definition")
	}
	fn resource(&self, entry: &Entry) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: entry.kind.clone(),
			id: entry.id.clone(),
			attributes: json!({"version":entry.version}),
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!((&*resource.tenant, &*resource.id), ("tenant", "embedding"));
		point(&self.0, format!("catalog.require:{action}")).await?;
		if self.0.0.lock().unwrap().denied.as_deref() == Some(action) {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		panic!("embedding requires explicit authority")
	}
	async fn active(&mut self, _: &Entry) -> Result<bool> {
		panic!("embedding does not discover installations")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("embedding compares the exact configuration")
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
			.calls
			.push(if self.committed { "drop" } else { "rollback" }.into());
	}
}
#[async_trait]
impl GenerationEmbeddingRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://origin"
	}
	async fn begin(&self) -> Result<Box<dyn GenerationEmbeddingSession>> {
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
impl GenerationEmbeddingSession for Session {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()> {
		point(&self.world, format!("charge:{}", request.as_u128())).await?;
		*self.ledger.balances.get_mut(&request).unwrap() += amount;
		Ok(())
	}
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()> {
		point(&self.world, format!("reserve:{}", request.as_u128())).await?;
		self.ledger.attempts.insert(request, attempt.clone());
		self.ledger.reports.insert(request, None);
		Ok(())
	}
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()> {
		point(&self.world, format!("refund:{}", request.as_u128())).await?;
		*self.ledger.balances.get_mut(&request).unwrap() -= amount;
		Ok(())
	}
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()> {
		assert_eq!(attempt, self.ledger.attempts[&request].id);
		point(&self.world, format!("report:{}", request.as_u128())).await?;
		*self.ledger.reports.get_mut(&request).unwrap() = reported;
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
async fn reserve_from(world: &World, origin: Origin) -> Result<Option<Reservation>> {
	reserve(
		&mut Authority(world.clone()),
		Arc::new(Repository(world.clone())),
		Uuid::from_u128(3),
		&config(),
		"日本",
		origin,
	)
	.await
}
async fn receipt(world: &World) -> Reservation {
	reserve_from(world, Origin::Query(Some(Uuid::from_u128(77))))
		.await
		.unwrap()
		.unwrap()
}
fn assert_empty(world: &World) {
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.commits), (0, 0));
	assert_eq!(
		state.ledger.balances.values().copied().collect::<Vec<_>>(),
		[0, 0]
	);
	assert_eq!(state.ledger.attempts.len(), 0);
	assert_eq!(state.ledger.reports.len(), 0);
}
fn assert_settled(world: &World, balance: i64, report: Option<i64>, commits: usize) {
	let state = world.0.lock().unwrap();
	assert_eq!((state.active, state.commits), (0, commits));
	for id in [Uuid::from_u128(11), Uuid::from_u128(22)] {
		assert_eq!(state.ledger.balances[&id], balance);
		assert_eq!(state.ledger.reports[&id], report);
	}
}

#[rstest]
#[case(Origin::Query(None))]
#[case(Origin::Query(Some(Uuid::from_u128(77))))]
#[case(Origin::Index(Uuid::from_u128(88)))]
#[tokio::test]
async fn authority_and_all_ancestors_commit_before_returning_a_receipt(
	world: World,
	#[case] origin: Origin,
) {
	let _receipt = reserve_from(&world, origin).await.unwrap().unwrap();
	assert_settled(&world, 1030, None, 1);
	let state = world.0.lock().unwrap();
	let first = &state.ledger.attempts[&Uuid::from_u128(11)];
	assert_eq!(first, &state.ledger.attempts[&Uuid::from_u128(22)]);
	assert_eq!(
		(
			first.workspace,
			first.origin,
			first.request_bytes,
			first.amount
		),
		(Uuid::from_u128(3), origin, 6, 1030)
	);
	assert_eq!(
		(&*first.provider.id, &*first.provider.version),
		("embedding", "1.0.0")
	);
	let begin = state.calls.iter().position(|call| call == "begin").unwrap();
	for required in [
		"live",
		"current:11",
		"current:22",
		"pinned:11",
		"pinned:22",
		"catalog.require:registry.read",
		"catalog.require:embedding.invoke",
	] {
		assert!(
			state
				.calls
				.iter()
				.position(|call| call == required)
				.unwrap() < begin,
			"{required} must precede accounting"
		);
	}
}

#[rstest]
#[tokio::test]
async fn ordinary_agents_do_not_acquire_generation_accounting(world: World) {
	world.0.lock().unwrap().jobs.clear();
	assert!(
		reserve_from(&world, Origin::Query(None))
			.await
			.unwrap()
			.is_none()
	);
	assert_empty(&world);
	assert_eq!(world.0.lock().unwrap().calls, ["requests"]);
}

#[rstest]
#[case("ancestor_tenant")]
#[case("ancestor_expired")]
#[case("ancestor_terminal")]
#[case("policy_disabled")]
#[case("first_expired")]
#[case("first_remote")]
#[case("missing_embedding_parent")]
#[case("provider_revision_parent")]
#[case("inherited_unapproved")]
#[case("catalog_missing")]
#[case("registry.read")]
#[case("embedding.invoke")]
#[case("wrong_kind")]
#[case("changed_configuration")]
#[tokio::test]
async fn rejected_authority_or_provider_pins_never_charge_ancestors(
	world: World,
	#[case] reason: &str,
) {
	{
		let mut state = world.0.lock().unwrap();
		match reason {
			"ancestor_tenant" => state.jobs[1].tenant = "other".into(),
			"ancestor_expired" => {
				state.jobs[1].expires_at = DateTime::from_timestamp(1500, 0).unwrap()
			}
			"ancestor_terminal" => state.jobs[1].status = "STOPPED".into(),
			"policy_disabled" => {
				state.enabled.insert(Uuid::from_u128(22), false);
			}
			"first_expired" => {
				state.jobs[0].expires_at = DateTime::from_timestamp(1500, 0).unwrap()
			}
			"first_remote" => state.jobs[0].home_node = "aidash://home".into(),
			"missing_embedding_parent" => {
				state
					.policies
					.get_mut(&Uuid::from_u128(22))
					.unwrap()
					.embedding = None
			}
			"provider_revision_parent" => {
				state
					.policies
					.get_mut(&Uuid::from_u128(22))
					.unwrap()
					.embedding
					.as_mut()
					.unwrap()
					.provider
					.version = "2.0.0".into()
			}
			"inherited_unapproved" => state.approved = false,
			"catalog_missing" => state.document = None,
			"registry.read" | "embedding.invoke" => state.denied = Some(reason.into()),
			"wrong_kind" => state.document.as_mut().unwrap()["kind"] = json!("model"),
			"changed_configuration" => {
				state.document.as_mut().unwrap()["config"]["model_version"] = json!("2")
			}
			_ => panic!("unknown rejection fixture"),
		}
	}
	let result = reserve_from(&world, Origin::Query(None)).await;
	if reason == "missing_embedding_parent" {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="generated semantic memory requires an approved embedding provider")
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_empty(&world);
	assert!(
		!world
			.0
			.lock()
			.unwrap()
			.calls
			.iter()
			.any(|call| call == "begin")
	);
}

#[rstest]
#[case(None, 1030, None, false)]
#[case(Some(0), 1030, Some(0), false)]
#[case(Some(10), 10, Some(10), false)]
#[case(Some(1030), 1030, Some(1030), false)]
#[case(Some(1031), 1030, Some(1031), true)]
#[case(Some(u64::MAX), 1030, None, true)]
#[tokio::test]
async fn settlement_commits_reports_even_when_usage_exceeds_the_receipt(
	world: World,
	#[case] reported: Option<u64>,
	#[case] balance: i64,
	#[case] stored: Option<i64>,
	#[case] exceeded: bool,
) {
	let receipt = receipt(&world).await;
	let result = receipt.settle(reported).await;
	if exceeded {
		assert!(
			matches!(result,Err(Error::Invalid(message)) if message=="embedding usage exceeded reserved input limits")
		);
	} else {
		result.unwrap();
	}
	assert_settled(&world, balance, stored, 2);
}

#[rstest]
#[case("begin")]
#[case("charge:11")]
#[case("reserve:11")]
#[case("charge:22")]
#[case("reserve:22")]
#[case("commit")]
#[tokio::test]
async fn reservation_faults_roll_back_every_ancestor(world: World, #[case] fault: &str) {
	world.0.lock().unwrap().fail = Some(fault.into());
	assert!(matches!(
		reserve_from(&world, Origin::Query(None)).await,
		Err(Error::External(_))
	));
	assert_empty(&world);
}

#[rstest]
#[case("begin")]
#[case("refund:11")]
#[case("report:11")]
#[case("refund:22")]
#[case("report:22")]
#[case("commit")]
#[tokio::test]
async fn settlement_faults_keep_the_original_charge_and_unreported_receipt(
	world: World,
	#[case] fault: &str,
) {
	let receipt = receipt(&world).await;
	world.0.lock().unwrap().fail = Some(fault.into());
	assert!(matches!(
		receipt.settle(Some(10)).await,
		Err(Error::External(_))
	));
	assert_settled(&world, 1030, None, 1);
}

#[rstest]
#[case("charge:11")]
#[case("charge:22")]
#[tokio::test]
async fn cancelled_reservation_drops_all_provisional_charges(world: World, #[case] pause: &str) {
	world.0.lock().unwrap().pause = Some(pause.into());
	{
		let mut future = Box::pin(reserve_from(&world, Origin::Query(None)));
		assert!(futures_util::poll!(&mut future).is_pending());
		assert_eq!(world.0.lock().unwrap().active, 1);
	}
	assert_empty(&world);
	assert_eq!(world.0.lock().unwrap().calls.last().unwrap(), "rollback");
}

#[rstest]
#[case("report:11")]
#[case("report:22")]
#[tokio::test]
async fn cancelled_settlement_retains_the_durable_charge(world: World, #[case] pause: &str) {
	let receipt = receipt(&world).await;
	world.0.lock().unwrap().pause = Some(pause.into());
	{
		let mut future = Box::pin(receipt.settle(Some(10)));
		assert!(futures_util::poll!(&mut future).is_pending());
		assert_eq!(world.0.lock().unwrap().active, 1);
	}
	assert_settled(&world, 1030, None, 1);
	assert_eq!(world.0.lock().unwrap().calls.last().unwrap(), "rollback");
}

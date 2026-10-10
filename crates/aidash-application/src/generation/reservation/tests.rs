use super::*;
use crate::{
	generation::test_support,
	ports::{catalog::CatalogScope, generation::reservation::GenerationReservationSession},
};
use aidash_domain::{
	generation::{
		policy::{Compaction, Embedding, Summary},
		remote::{Allowance, Approvals, Attempt, ReservationBinding},
	},
	policy::Resource,
	registry::{EntityRef, Entry},
	semantic::remote::Provider,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
use uuid::Uuid;

#[fixture]
fn usage() -> Usage {
	Usage {
		operation_id: Uuid::from_u128(1),
		attempt_id: Uuid::from_u128(2),
		dispatcher_node: "aidash://edge".into(),
		grant_id: Uuid::from_u128(3),
		admission_id: Uuid::from_u128(4),
		purpose: Purpose::Inference,
		provider: Provider {
			node_id: "aidash://edge".into(),
			entry: EntityRef {
				id: "model".into(),
				version: "1.0.0".into(),
			},
			digest: format!("sha256:{}", "a".repeat(64)),
			configuration_digest: format!("sha256:{}", "b".repeat(64)),
		},
		input_digest: format!("sha256:{}", "c".repeat(64)),
		reserved_tokens: 100,
	}
}
struct State {
	jobs: Vec<Request>,
	spec: Value,
	enabled: BTreeMap<Uuid, bool>,
	document: Option<Value>,
	inherited: bool,
	approved: bool,
	attempt: Attempt,
	existing: BTreeMap<Uuid, ReservationBinding>,
	rows: BTreeMap<Uuid, u64>,
	calls: Vec<String>,
	committed: Vec<String>,
	resources: Vec<Resource>,
	fail: Option<String>,
	denied: Option<String>,
	pause: Option<String>,
	active: usize,
}
#[derive(Clone)]
struct World(Arc<Mutex<State>>);
#[fixture]
fn world(usage: Usage) -> World {
	let mut jobs = vec![test_support::request(11), test_support::request(22)];
	for job in &mut jobs {
		job.expires_at = DateTime::from_timestamp(2000, 0).unwrap();
	}
	let mut spec: Spec = serde_json::from_value(test_support::specification()).unwrap();
	spec.embedding = Some(Embedding {
		provider: usage.provider.entry.clone(),
		calls_per_agent: 2,
		call_budget: 4,
	});
	spec.compaction = Some(Compaction {
		provider: usage.provider.entry.clone(),
		calls_per_agent: 2,
		call_budget: 4,
	});
	spec.summary = Some(Summary {
		provider: usage.provider.entry.clone(),
		calls_per_agent: 2,
		call_budget: 4,
	});
	let allowance = Allowance {
		provider: usage.provider.clone(),
		calls_per_agent: 2,
		call_budget: 4,
	};
	spec.remote = Some(Approvals {
		memory: vec![],
		inference: vec![usage.provider.clone()],
		embedding: Some(allowance.clone()),
		compaction: Some(allowance.clone()),
		summary: Some(allowance),
	});
	let document = json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":""},"config":{"model":"fixture-model"}});
	World(Arc::new(Mutex::new(State {
		jobs,
		spec: json!(spec),
		enabled: BTreeMap::new(),
		document: Some(document),
		inherited: false,
		approved: true,
		attempt: Attempt {
			digest: usage.digest().unwrap(),
			result: None,
		},
		existing: BTreeMap::new(),
		rows: BTreeMap::new(),
		calls: vec![],
		committed: vec![],
		resources: vec![],
		fail: None,
		denied: None,
		pause: None,
		active: 0,
	})))
}
impl World {
	fn calls(&self) -> Vec<String> {
		self.0.lock().unwrap().calls.clone()
	}
	fn bind(&self, usage: &Usage) {
		self.0.lock().unwrap().attempt.digest = usage.digest().unwrap();
	}
	fn local(&self, usage: &mut Usage) {
		usage.provider.node_id = "aidash://origin".into();
		usage.dispatcher_node.clone_from(&usage.provider.node_id);
		let entry: Entry =
			serde_json::from_value(self.0.lock().unwrap().document.clone().unwrap()).unwrap();
		usage.provider.digest = digest(&json!(entry));
		usage.provider.configuration_digest = digest(&entry.config);
		self.bind(usage);
	}
	fn clean(&self) {
		let state = self.0.lock().unwrap();
		assert_eq!(state.active, 0);
		assert!(state.committed.is_empty());
	}
}
async fn point(world: &World, name: String) -> Result<()> {
	let (fail, denied, pause) = {
		let mut state = world.0.lock().unwrap();
		state.calls.push(name.clone());
		(
			state.fail.as_ref() == Some(&name),
			state.denied.as_ref() == Some(&name),
			state.pause.as_ref() == Some(&name),
		)
	};
	if denied {
		return Err(Error::Forbidden);
	}
	if fail {
		return Err(Error::External("adapter failure".into()));
	}
	if pause {
		std::future::pending::<()>().await;
	}
	Ok(())
}
fn resource(kind: &str, id: &str, attributes: Value) -> Resource {
	Resource {
		tenant: "tenant".into(),
		kind: kind.into(),
		id: id.into(),
		attributes,
	}
}
struct Authority(World);
#[async_trait]
impl GenerationUsageAuthority for Authority {
	fn now(&self) -> DateTime<Utc> {
		DateTime::from_timestamp(1500, 0).unwrap()
	}
	async fn jobs(&mut self, node: &str) -> Result<Vec<Request>> {
		assert_eq!(node, "aidash://origin");
		point(&self.0, "jobs".into()).await?;
		Ok(self.0.0.lock().unwrap().jobs.clone())
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		point(&self.0, format!("current:{}", job.id)).await?;
		Ok(*self
			.0
			.0
			.lock()
			.unwrap()
			.enabled
			.get(&job.id)
			.unwrap_or(&true))
	}
	async fn pinned_policy(&mut self, job: &Request) -> Result<Value> {
		assert_eq!(job.policy_revision, 7);
		point(&self.0, format!("pinned:{}", job.id)).await?;
		Ok(self.0.0.lock().unwrap().spec.clone())
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		self
	}
	fn remote_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		let resource = resource(kind, id, attributes);
		self.0.0.lock().unwrap().resources.push(resource.clone());
		resource
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(resource.tenant, "tenant");
		point(&self.0, action.into()).await
	}
}
#[async_trait]
impl CatalogScope for Authority {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn inherited_lease(&self) -> bool {
		self.0.0.lock().unwrap().inherited
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		assert_eq!(reference.id, "model");
		self.0.0.lock().unwrap().approved
	}
	fn remember(&mut self, _: &EntityRef) {
		self.0.0.lock().unwrap().calls.push("remember".into());
	}
	async fn distribution_lock(&mut self) -> Result<()> {
		point(&self.0, "catalog.lock".into()).await
	}
	async fn document(&mut self, reference: &EntityRef) -> Result<Option<Value>> {
		assert_eq!(reference.id, "model");
		point(&self.0, "catalog.document".into()).await?;
		Ok(self.0.0.lock().unwrap().document.clone())
	}
	async fn documents(&mut self) -> Result<Vec<Value>> {
		panic!("reservation cannot discover the catalog")
	}
	fn resource(&self, entry: &Entry) -> Resource {
		resource(&entry.kind, &entry.id, json!({"version":entry.version}))
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		GenerationUsageAuthority::require(self, resource, action).await
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		panic!("reservation requires exact permission")
	}
	async fn active(&mut self, _: &Entry) -> Result<bool> {
		panic!("reservation cannot discover installations")
	}
	async fn check_pinned(&mut self, _: &Entry) -> Result<()> {
		panic!("reservation compares the exact provider descriptor")
	}
}
struct Repository(World);
struct Session {
	world: World,
	existing: BTreeMap<Uuid, ReservationBinding>,
	effects: Vec<String>,
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
impl GenerationReservationRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://origin"
	}
	async fn begin(&self) -> Result<Box<dyn GenerationReservationSession>> {
		point(&self.0, "begin".into()).await?;
		let mut state = self.0.0.lock().unwrap();
		state.active += 1;
		Ok(Box::new(Session {
			world: self.0.clone(),
			existing: state.existing.clone(),
			effects: vec![],
			committed: false,
		}))
	}
}
#[async_trait]
impl GenerationReservationSession for Session {
	async fn lock_attempt(&mut self, attempt: Uuid, _: &str) -> Result<Attempt> {
		assert_eq!(attempt, Uuid::from_u128(2));
		point(&self.world, "attempt".into()).await?;
		Ok(self.world.0.lock().unwrap().attempt.clone())
	}
	async fn lock_budget(&mut self, job: &Request) -> Result<()> {
		point(&self.world, format!("budget:{}", job.id)).await
	}
	async fn existing(
		&mut self,
		job: &Request,
		usage: &Usage,
	) -> Result<Option<ReservationBinding>> {
		assert_eq!(usage.attempt_id, Uuid::from_u128(2));
		point(&self.world, format!("existing:{}", job.id)).await?;
		Ok(self.existing.get(&job.id).cloned())
	}
	async fn debit_budget(&mut self, job: &Request, usage: &Usage) -> Result<u64> {
		point(&self.world, format!("debit:{}", job.id)).await?;
		self.effects.push(format!(
			"debit:{}:{}:{}",
			job.id,
			usage.reserved_tokens,
			usage.purpose.name()
		));
		Ok(*self.world.0.lock().unwrap().rows.get(&job.id).unwrap_or(&1))
	}
	async fn insert(&mut self, job: &Request, usage: &Usage, digest: &str) -> Result<()> {
		assert_eq!(digest, usage.digest()?);
		point(&self.world, format!("insert:{}", job.id)).await?;
		self.existing.insert(
			job.id,
			ReservationBinding {
				digest: digest.into(),
				state: "RESERVED".into(),
			},
		);
		self.effects.push(format!("insert:{}", job.id));
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		point(&self.world, "commit".into()).await?;
		{
			let mut state = self.world.0.lock().unwrap();
			state.committed.append(&mut self.effects);
			state.existing = self.existing.clone();
		}
		self.committed = true;
		Ok(())
	}
}
fn adapters(world: &World) -> (Authority, Repository) {
	(Authority(world.clone()), Repository(world.clone()))
}

#[rstest]
#[case(("ACTIVE",false,"",true,false,2000,true))]
#[case(("QUEUED",true,"aidash://home",true,false,2000,true))]
#[case(("QUEUED",false,"aidash://home",true,false,2000,false))]
#[case(("QUEUED",true,"",true,false,2000,false))]
#[case(("PENDING_APPROVAL",true,"aidash://home",true,false,2000,false))]
#[case(("FAILED",true,"aidash://home",true,false,2000,false))]
#[case(("ACTIVE",true,"",false,false,2000,false))]
#[case(("ACTIVE",true,"",true,true,2000,false))]
#[case(("ACTIVE",true,"",true,false,1500,false))]
#[case(("ACTIVE",true,"",true,false,1499,false))]
#[tokio::test]
async fn lineage_requires_current_live_authority(
	world: World,
	#[case] facts: (&str, bool, &str, bool, bool, i64, bool),
) {
	let (status, prepared, home, enabled, released, expiry, allowed) = facts;
	{
		let mut state = world.0.lock().unwrap();
		let job = &mut state.jobs[1];
		job.status = status.into();
		job.prepared = prepared;
		job.home_node = home.into();
		job.quota_released = released;
		job.expires_at = DateTime::from_timestamp(expiry, 0).unwrap();
		state.enabled.insert(Uuid::from_u128(22), enabled);
	}
	let (mut authority, _) = adapters(&world);
	let result = lineage(&mut authority, "aidash://origin").await;
	if allowed {
		let owners = result.unwrap();
		assert_eq!(owners.len(), 2);
		assert_eq!(owners[0].request_id, Uuid::from_u128(11));
		assert_eq!(owners[1].request_id, Uuid::from_u128(22));
		assert_eq!(owners[1].node_id, "aidash://origin");
		assert_eq!(owners[1].policy_revision, 7);
	} else {
		assert!(matches!(
			result,
			Err(Error::RemoteSemantic(Failure::Authority))
		));
	}
	assert_eq!(
		world.calls(),
		vec![
			"jobs".to_owned(),
			format!("current:{}", Uuid::from_u128(11)),
			format!("current:{}", Uuid::from_u128(22))
		]
	);
	world.clean();
}
#[rstest]
#[case("QUEUED")]
#[case("PENDING_APPROVAL")]
#[case("STOPPED")]
#[tokio::test]
async fn prepared_lineage_does_not_grant_provider_usage(
	world: World,
	usage: Usage,
	#[case] status: &str,
) {
	{
		let mut state = world.0.lock().unwrap();
		state.jobs[1].status = status.into();
		state.jobs[1].home_node = "aidash://home".into();
	}
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::RemoteSemantic(Failure::Authority))
	));
	assert!(!world.calls().contains(&"begin".into()));
	world.clean();
}
#[rstest]
#[case(Purpose::Inference)]
#[case(Purpose::Embedding)]
#[case(Purpose::Compaction)]
#[case(Purpose::Summary)]
#[tokio::test]
async fn all_ancestors_are_authorized_before_atomic_debits_and_exact_replay(
	world: World,
	mut usage: Usage,
	#[case] purpose: Purpose,
) {
	usage.purpose = purpose;
	world.bind(&usage);
	let (mut authority, repository) = adapters(&world);
	let receipts = reserve(&mut authority, &repository, &usage).await.unwrap();
	let owners = lineage(&mut authority, "aidash://origin").await.unwrap();
	aidash_domain::generation::remote::verify_receipts(&owners, &receipts, &usage).unwrap();
	let calls = world.calls();
	let begin = calls.iter().position(|v| v == "begin").unwrap();
	assert_eq!(
		calls[..begin]
			.iter()
			.filter(|v| v.as_str() == purpose.action())
			.count(),
		2
	);
	let mut expected = vec!["attempt".to_owned()];
	for id in [11, 22] {
		for name in ["budget", "existing", "debit", "insert"] {
			expected.push(format!("{name}:{}", Uuid::from_u128(id)));
		}
	}
	expected.extend(["commit".into(), "drop".into()]);
	assert_eq!(&calls[begin + 1..begin + 1 + expected.len()], expected);
	let committed = world.0.lock().unwrap().committed.clone();
	assert_eq!(committed.len(), 4);
	assert_eq!(
		committed[0],
		format!("debit:{}:100:{}", Uuid::from_u128(11), purpose.name())
	);
	let replay = reserve(&mut authority, &repository, &usage).await.unwrap();
	assert_eq!(json!(replay), json!(receipts));
	assert_eq!(world.0.lock().unwrap().committed, committed);
	assert_eq!(world.0.lock().unwrap().active, 0);
	for resource in &world.0.lock().unwrap().resources {
		assert_eq!(resource.id, "aidash://edge/registry/model@1.0.0");
		assert_eq!(
			resource.attributes,
			json!({"remote_node":"aidash://edge","definition_digest":usage.provider.digest,"purpose":purpose.name()})
		);
	}
}
#[rstest]
#[case(Purpose::Inference)]
#[case(Purpose::Embedding)]
#[case(Purpose::Compaction)]
#[case(Purpose::Summary)]
#[tokio::test]
async fn local_providers_require_exact_catalog_use_then_read(
	world: World,
	mut usage: Usage,
	#[case] purpose: Purpose,
) {
	usage.purpose = purpose;
	world.local(&mut usage);
	let (mut authority, repository) = adapters(&world);
	reserve(&mut authority, &repository, &usage).await.unwrap();
	let calls = world.calls();
	let document = calls.iter().position(|v| v == "catalog.document").unwrap();
	assert_eq!(
		&calls[document + 1..document + 4],
		&[purpose.action(), "remember", "registry.read"]
	);
	assert!(world.0.lock().unwrap().resources.is_empty());
	assert_eq!(world.0.lock().unwrap().committed.len(), 4);
}
#[rstest]
#[case("registry.read")]
#[case("model.infer")]
#[tokio::test]
async fn both_remote_permissions_are_required_before_begin(
	world: World,
	usage: Usage,
	#[case] action: &str,
) {
	world.0.lock().unwrap().denied = Some(action.into());
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::Forbidden)
	));
	assert!(!world.calls().contains(&"begin".into()));
	assert!(!world.calls().contains(&"catalog.document".into()));
	world.clean();
}
#[rstest]
#[case("entry")]
#[case("definition")]
#[case("configuration")]
#[tokio::test]
async fn remote_approvals_bind_the_exact_descriptor(
	world: World,
	mut usage: Usage,
	#[case] changed: &str,
) {
	match changed {
		"entry" => usage.provider.entry.version = "2.0.0".into(),
		"definition" => usage.provider.digest = format!("sha256:{}", "d".repeat(64)),
		"configuration" => {
			usage.provider.configuration_digest = format!("sha256:{}", "d".repeat(64))
		}
		_ => unreachable!(),
	}
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::RemoteSemantic(Failure::Allowance))
	));
	assert!(!world.calls().contains(&"registry.read".into()));
	assert!(!world.calls().contains(&"begin".into()));
	world.clean();
}
#[rstest]
#[case("definition")]
#[case("configuration")]
#[tokio::test]
async fn local_configuration_changes_cannot_open_a_debit(
	world: World,
	mut usage: Usage,
	#[case] changed: &str,
) {
	world.local(&mut usage);
	if changed == "definition" {
		usage.provider.digest = format!("sha256:{}", "d".repeat(64));
	} else {
		usage.provider.configuration_digest = format!("sha256:{}", "d".repeat(64));
	}
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::RemoteSemantic(Failure::Configuration))
	));
	assert!(world.calls().contains(&"registry.read".into()));
	assert!(!world.calls().contains(&"begin".into()));
	world.clean();
}
#[rstest]
#[tokio::test]
async fn inherited_catalog_approval_is_applied_to_local_worker_usage(
	world: World,
	mut usage: Usage,
) {
	world.local(&mut usage);
	{
		let mut state = world.0.lock().unwrap();
		state.inherited = true;
		state.approved = false;
	}
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::Forbidden)
	));
	assert!(!world.calls().contains(&"catalog.document".into()));
	assert!(!world.calls().contains(&"begin".into()));
	world.clean();
}
#[rstest]
#[case("operation")]
#[case("tokens")]
#[case("digest")]
#[tokio::test]
async fn invalid_usage_precedes_every_authority_and_database_operation(
	world: World,
	mut usage: Usage,
	#[case] changed: &str,
) {
	match changed {
		"operation" => usage.operation_id = Uuid::nil(),
		"tokens" => usage.reserved_tokens = 0,
		"digest" => usage.input_digest = "invalid".into(),
		_ => unreachable!(),
	}
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::RemoteSemantic(Failure::ProviderContract))
	));
	assert!(world.calls().is_empty());
	world.clean();
}
#[rstest]
#[case(0)]
#[case(2)]
#[tokio::test]
async fn a_later_ancestor_cas_failure_rolls_back_every_previous_debit(
	world: World,
	usage: Usage,
	#[case] rows: u64,
) {
	world
		.0
		.lock()
		.unwrap()
		.rows
		.insert(Uuid::from_u128(22), rows);
	let (mut authority, repository) = adapters(&world);
	assert!(matches!(
		reserve(&mut authority, &repository, &usage).await,
		Err(Error::RemoteSemantic(Failure::Allowance))
	));
	assert!(
		world
			.calls()
			.contains(&format!("insert:{}", Uuid::from_u128(11)))
	);
	assert_eq!(world.calls().last().unwrap(), "rollback");
	world.clean();
}
#[rstest]
#[case("SETTLED", false)]
#[case("RELEASED", false)]
#[case("RESERVED", true)]
#[tokio::test]
async fn changed_or_final_reservations_cannot_be_debited_again_or_rebound(
	world: World,
	usage: Usage,
	#[case] state: &str,
	#[case] changed: bool,
) {
	world.0.lock().unwrap().existing.insert(
		Uuid::from_u128(11),
		ReservationBinding {
			digest: if changed {
				"different".into()
			} else {
				usage.digest().unwrap()
			},
			state: state.into(),
		},
	);
	let (mut authority, repository) = adapters(&world);
	let error = reserve(&mut authority, &repository, &usage)
		.await
		.unwrap_err();
	assert!(
		matches!(error,Error::Conflict(ref message) if message=="provider attempt already has a different or final reservation")
	);
	assert!(
		!world
			.calls()
			.contains(&format!("debit:{}", Uuid::from_u128(11)))
	);
	world.clean();
}
#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn terminal_attempt_or_changed_binding_precedes_any_budget_lock(
	world: World,
	usage: Usage,
	#[case] changed: bool,
) {
	{
		let mut state = world.0.lock().unwrap();
		if changed {
			state.attempt.digest = "different".into();
		} else {
			state.attempt.result = Some(json!({"state":"aborted"}));
		}
	}
	let (mut authority, repository) = adapters(&world);
	let error = reserve(&mut authority, &repository, &usage)
		.await
		.unwrap_err();
	assert!(
		matches!(error,Error::Conflict(ref message) if message==if changed{"provider attempt has a different reservation"}else{"provider attempt already finalized"})
	);
	assert!(!world.calls().iter().any(|name| name.starts_with("budget:")));
	world.clean();
}
#[rstest]
#[case("begin")]
#[case("attempt")]
#[case("budget")]
#[case("existing")]
#[case("debit")]
#[case("insert")]
#[case("commit")]
#[tokio::test]
async fn adapter_failures_return_no_receipts_or_partial_debits(
	world: World,
	usage: Usage,
	#[case] phase: &str,
) {
	let phase = if ["budget", "existing", "debit", "insert"].contains(&phase) {
		format!("{phase}:{}", Uuid::from_u128(22))
	} else {
		phase.into()
	};
	world.0.lock().unwrap().fail = Some(phase);
	let (mut authority, repository) = adapters(&world);
	assert!(
		matches!(reserve(&mut authority,&repository,&usage).await,Err(Error::External(ref message)) if message=="adapter failure")
	);
	world.clean();
}
#[rstest]
#[case("attempt")]
#[case("debit")]
#[case("insert")]
#[case("commit")]
#[tokio::test]
async fn cancellation_drops_owned_transactions_and_provisional_ancestor_debits(
	world: World,
	usage: Usage,
	#[case] phase: &str,
) {
	let phase = if ["debit", "insert"].contains(&phase) {
		format!("{phase}:{}", Uuid::from_u128(22))
	} else {
		phase.into()
	};
	world.0.lock().unwrap().pause = Some(phase);
	let (mut authority, repository) = adapters(&world);
	{
		let future = reserve(&mut authority, &repository, &usage);
		tokio::pin!(future);
		assert!(futures_util::poll!(future.as_mut()).is_pending());
		assert_eq!(world.0.lock().unwrap().active, 1);
	}
	assert_eq!(world.calls().last().unwrap(), "rollback");
	world.clean();
}
#[rstest]
#[tokio::test]
async fn even_an_empty_lineage_serializes_the_attempt_before_returning(world: World, usage: Usage) {
	world.0.lock().unwrap().jobs.clear();
	let (mut authority, repository) = adapters(&world);
	assert!(
		reserve(&mut authority, &repository, &usage)
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		world.calls(),
		vec!["jobs", "begin", "attempt", "commit", "drop"]
	);
	world.clean();
}

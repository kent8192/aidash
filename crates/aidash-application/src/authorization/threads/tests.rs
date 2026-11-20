use super::*;
use crate::{authorization::Snapshot, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{
	RunMetadata,
	policy::{Decision, Evaluation},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
struct Scope {
	snapshot: Snapshot,
	subjects: Vec<String>,
	environment: Value,
	context: Value,
	inherited: bool,
	rechecking: Vec<Uuid>,
	owner: Option<String>,
	/// The Duty whose own Thread each listed or read Thread is.
	duties: std::collections::BTreeMap<Uuid, Uuid>,
	rows: Vec<(Uuid, String)>,
	run: Option<Run>,
	run_visible: bool,
	admitted: bool,
	require_allowed: bool,
	calls: Vec<String>,
	owner_lookups: Vec<(Uuid, bool)>,
	records: Vec<(Evaluation, Decision)>,
	fail: Option<&'static str>,
	/// The Node every Thread of this fake belongs to.
	node: String,
	/// A worker's execution grant reading for a Run.
	reads_for_run: bool,
}
fn run() -> Run {
	Run {
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: Uuid::from_u128(9),
		task_id: Uuid::from_u128(10),
		thread_id: Uuid::from_u128(1),
		home_node: "aidash://local".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: Default::default(),
		state: Default::default(),
		recovery: Default::default(),
		control: aidash_domain::RunControl::Active,
		context: Default::default(),
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
#[fixture]
fn scope() -> Scope {
	Scope{snapshot:Snapshot{revision:31,bundle:serde_json::from_value(json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"}},"policies":[{"id":"read","effect":"allow","subjects":{"any":true},"actions":["thread.read"],"resources":{"kinds":["thread"],"ids":[Uuid::from_u128(1),Uuid::from_u128(2),Uuid::from_u128(3)]}},{"id":"events","effect":"allow","subjects":{"any":true},"actions":["thread.events"],"resources":{"kinds":["thread"],"ids":[Uuid::from_u128(1),Uuid::from_u128(3)]}}]})).unwrap()},subjects:vec!["reader".into()],environment:json!({"transport":"worker"}),context:json!({"thread_id":Uuid::from_u128(1),"created_by":"lease-default"}),inherited:false,rechecking:vec![],owner:Some("stored-owner".into()),duties:Default::default(),rows:vec![(Uuid::from_u128(3),"third-owner".into()),(Uuid::from_u128(2),"second-owner".into()),(Uuid::from_u128(4),"fourth-owner".into()),(Uuid::from_u128(1),"first-owner".into())],run:Some(run()),run_visible:true,admitted:false,require_allowed:true,calls:vec![],owner_lookups:vec![],records:vec![],fail:None,node:"home-node".into(),reads_for_run:false}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			Err(Error::Port(Box::new(std::io::Error::other(name))))
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl AuthorizationLease for Scope {
	fn snapshot(&self) -> &Snapshot {
		&self.snapshot
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn environment(&self) -> &Value {
		&self.environment
	}
	async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
		self.touch("audit")?;
		self.records.extend_from_slice(records);
		Ok(())
	}
}
#[async_trait]
impl ThreadResourceScope for Scope {
	fn inherited(&self) -> bool {
		self.inherited
	}
	fn context(&self) -> &Value {
		&self.context
	}
	fn recheck_reaches(&self, id: Uuid) -> bool {
		self.rechecking.contains(&id)
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		super::super::lease::resource("tenant", &self.context, kind, id, attributes)
	}
	async fn owner(&mut self, id: Uuid, lock: bool) -> Result<Option<ThreadOwner>> {
		self.touch("owner")?;
		self.owner_lookups.push((id, lock));
		Ok(self.owner.clone().map(|owner| ThreadOwner {
			owner,
			duty_id: self.duties.get(&id).copied(),
		}))
	}
}
impl Scope {
	fn owned_rows(&self) -> Vec<(Uuid, ThreadOwner)> {
		self.rows
			.iter()
			.map(|(id, owner)| {
				(
					*id,
					ThreadOwner {
						owner: owner.clone(),
						duty_id: self.duties.get(id).copied(),
					},
				)
			})
			.collect()
	}
}
#[async_trait]
impl ThreadAuthorityScope for Scope {
	async fn locked_owner(&mut self, id: Uuid) -> Result<Option<ThreadOwner>> {
		self.owner(id, true).await
	}
	fn identity(&self) -> (&str, &str) {
		("tenant", "reader")
	}
	async fn all_owners(&mut self) -> Result<Vec<(Uuid, ThreadOwner)>> {
		self.touch("all_owners")?;
		Ok(self.owned_rows())
	}
	async fn event_owners(&mut self, selected: Option<Uuid>) -> Result<Vec<(Uuid, ThreadOwner)>> {
		self.touch("event_owners")?;
		Ok(self
			.owned_rows()
			.into_iter()
			.filter(|(id, _)| selected.is_none_or(|selected| selected == *id))
			.collect())
	}
	async fn thread_node(&mut self, _id: Uuid) -> Result<String> {
		Ok(self.node.clone())
	}
	fn reads_for_run(&self) -> bool {
		self.reads_for_run
	}
}
#[async_trait]
impl RunInteractionScope for Scope {
	async fn run(&mut self, id: Uuid) -> Result<Option<Run>> {
		assert_eq!(id, Uuid::from_u128(9));
		self.touch("run")?;
		Ok(self.run.clone())
	}
	fn set_context(&mut self, context: Value) {
		self.calls.push("context".into());
		self.context = context;
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(action, "thread.read");
		assert_eq!(resource.attributes["owner"], "stored-owner");
		self.touch("require")?;
		if self.require_allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		assert_eq!(run.id, Uuid::from_u128(9));
		// An admitted Run is checked through its admission, with no Thread context.
		if !self.admitted {
			assert_eq!(self.context["owner"], "stored-owner");
		}
		self.touch("run_visible")?;
		Ok(self.run_visible)
	}
	async fn admitted_here(&mut self, run: &RunMetadata) -> Result<bool> {
		assert_eq!(run.id, Uuid::from_u128(9));
		self.touch("admitted")?;
		Ok(self.admitted)
	}
}
#[rstest]
#[case::ordinary(false, true)]
#[case::inherited(true, false)]
#[tokio::test]
async fn thread_resource_retains_stored_owner_context_and_lock_mode(
	mut scope: Scope,
	#[case] inherited: bool,
	#[case] lock: bool,
) {
	scope.inherited = inherited;
	let id = Uuid::from_u128(1);
	let resource = resource(&mut scope, id).await.unwrap();
	assert_eq!(
		resource.attributes,
		json!({"owner":"stored-owner","thread_id":id,"created_by":"lease-default"})
	);
	assert_eq!(scope.owner_lookups, vec![(id, lock)]);
}
#[rstest]
#[case::different(json!({"thread_id":Uuid::from_u128(2)}))]
#[case::missing(json!({}))]
#[case::malformed(json!({"thread_id":1}))]
#[tokio::test]
async fn inherited_resource_cannot_expand_its_thread_before_any_lookup(
	mut scope: Scope,
	#[case] context: Value,
) {
	scope.inherited = true;
	scope.context = context;
	assert!(matches!(
		resource(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn inherited_resource_reaches_only_what_a_read_recheck_opened_and_takes_no_lock(
	mut scope: Scope,
) {
	let own = Uuid::from_u128(1);
	let opened = Uuid::from_u128(2);
	let other = Uuid::from_u128(3);
	scope.inherited = true;
	scope.context = json!({"thread_id":own});
	scope.rechecking = vec![opened];
	resource(&mut scope, opened).await.unwrap();
	assert_eq!(scope.owner_lookups, vec![(opened, false)]);
	assert!(matches!(
		resource(&mut scope, other).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.owner_lookups, vec![(opened, false)]);
}
#[rstest]
#[tokio::test]
async fn missing_thread_resource_is_a_denial(mut scope: Scope) {
	scope.owner = None;
	assert!(matches!(
		resource(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["owner"]);
}
#[rstest]
#[tokio::test]
async fn missing_ownership_overrides_allow_and_keeps_the_unavailable_audit(mut scope: Scope) {
	scope.owner = None;
	assert!(
		!allowed(&mut scope, Uuid::from_u128(1), "thread.read")
			.await
			.unwrap()
	);
	assert_eq!(scope.owner_lookups, vec![(Uuid::from_u128(1), true)]);
	assert_eq!(scope.calls, vec!["owner", "audit"]);
	let (input, decision) = &scope.records[0];
	assert_eq!(input.resource.attributes, json!({}));
	assert_eq!(decision.reason, "resource_unavailable");
	assert_eq!(decision.revision, 31);
	assert_eq!(decision.matched_policies, vec!["read"]);
	assert!(!decision.allowed);
}
#[rstest]
#[tokio::test]
async fn ownership_decision_uses_locked_identity_without_context_attribute_defaults(
	mut scope: Scope,
) {
	assert!(
		allowed(&mut scope, Uuid::from_u128(1), "thread.read")
			.await
			.unwrap()
	);
	let (input, decision) = &scope.records[0];
	assert_eq!(input.subject, "reader");
	assert_eq!(input.environment, json!({"transport":"worker"}));
	assert_eq!(
		input.resource.attributes,
		json!({"owner":"stored-owner","thread_id":Uuid::from_u128(1)})
	);
	assert!(decision.allowed);
	assert_eq!(decision.revision, 31);
}
#[rstest]
#[tokio::test]
async fn visible_threads_keep_repository_order_and_audit_each_row(mut scope: Scope) {
	assert_eq!(
		visible(&mut scope, "thread.read").await.unwrap(),
		vec![Uuid::from_u128(3), Uuid::from_u128(2), Uuid::from_u128(1)]
	);
	// The denied row also records whether the reader administers its Node.
	assert_eq!(scope.records.len(), 5);
	assert_eq!(
		scope
			.records
			.iter()
			.filter(|(input, _)| input.action == "thread.read")
			.map(|(input, _)| input.resource.attributes["owner"].as_str().unwrap())
			.collect::<Vec<_>>(),
		vec!["third-owner", "second-owner", "fourth-owner", "first-owner"]
	);
}
#[rstest]
#[tokio::test]
async fn event_threads_require_read_then_events_without_refetching_owners(mut scope: Scope) {
	assert_eq!(
		event_threads(&mut scope, None).await.unwrap(),
		vec![Uuid::from_u128(3), Uuid::from_u128(1)]
	);
	assert!(scope.owner_lookups.is_empty());
	assert_eq!(
		scope
			.records
			.iter()
			.map(|(input, _)| (input.resource.id.clone(), input.action.clone()))
			.collect::<Vec<_>>(),
		vec![
			(Uuid::from_u128(3).to_string(), "thread.read".into()),
			(Uuid::from_u128(3).to_string(), "thread.events".into()),
			(Uuid::from_u128(2).to_string(), "thread.read".into()),
			(Uuid::from_u128(2).to_string(), "thread.events".into()),
			("home-node".into(), "node.administer".into()),
			(Uuid::from_u128(4).to_string(), "thread.read".into()),
			("home-node".into(), "node.administer".into()),
			(Uuid::from_u128(1).to_string(), "thread.read".into()),
			(Uuid::from_u128(1).to_string(), "thread.events".into())
		]
	);
}
#[rstest]
#[tokio::test]
async fn selected_missing_thread_records_unavailability_without_disclosing_rows(mut scope: Scope) {
	scope.rows.clear();
	assert!(
		event_threads(&mut scope, Some(Uuid::from_u128(1)))
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(scope.calls, vec!["event_owners", "audit"]);
	assert_eq!(scope.records[0].1.reason, "resource_unavailable");
	assert_eq!(scope.records[0].0.resource.attributes, json!({}));
}
#[rstest]
#[tokio::test]
async fn thread_requirement_keeps_the_public_denial_after_auditing(mut scope: Scope) {
	assert!(matches!(
		require(&mut scope, Uuid::from_u128(4), "thread.read").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["owner", "audit"]);
	assert!(!scope.records[0].1.allowed);
}
#[rstest]
#[tokio::test]
async fn interaction_sets_saved_thread_context_before_read_policy_and_run_provenance(
	mut scope: Scope,
) {
	let result = run_for_interaction(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap();
	assert_eq!(result.id, Uuid::from_u128(9));
	assert_eq!(
		scope.calls,
		vec!["run", "owner", "context", "require", "run_visible"]
	);
	assert_eq!(scope.context["owner"], "stored-owner");
}
#[rstest]
#[case::run(0,vec!["run"])]
#[case::thread(1,vec!["run","owner"])]
#[case::read_policy(2,vec!["run","owner","context","require"])]
#[case::provenance(3,vec!["run","owner","context","require","run_visible"])]
#[tokio::test]
async fn interaction_denial_stops_at_the_original_dependency(
	mut scope: Scope,
	#[case] boundary: u8,
	#[case] calls: Vec<&str>,
) {
	match boundary {
		0 => scope.run = None,
		1 => scope.owner = None,
		2 => scope.require_allowed = false,
		_ => scope.run_visible = false,
	};
	assert!(matches!(
		run_for_interaction(&mut scope, Uuid::from_u128(9)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, calls);
}
#[rstest]
#[tokio::test]
async fn a_run_admitted_here_is_checked_through_its_admission_and_never_its_thread(
	mut scope: Scope,
) {
	scope.admitted = true;
	// The Home's Thread is not on this Node: no owner, context or thread.read.
	scope.owner = None;
	scope.require_allowed = false;
	let result = run_for_action(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap();
	assert_eq!(result.id, Uuid::from_u128(9));
	assert_eq!(scope.calls, vec!["run", "admitted", "run_visible"]);
}
#[rstest]
#[tokio::test]
async fn an_admitted_run_whose_chain_no_longer_reads_is_refused(mut scope: Scope) {
	scope.admitted = true;
	scope.run_visible = false;
	assert!(matches!(
		run_for_action(&mut scope, Uuid::from_u128(9)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["run", "admitted", "run_visible"]);
}
#[rstest]
#[tokio::test]
async fn a_run_without_an_admission_keeps_the_thread_interaction(mut scope: Scope) {
	let result = run_for_action(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap();
	assert_eq!(result.id, Uuid::from_u128(9));
	assert_eq!(
		scope.calls,
		vec![
			"run",
			"admitted",
			"owner",
			"context",
			"require",
			"run_visible"
		]
	);
}
#[rstest]
#[tokio::test]
async fn an_unknown_run_is_refused_before_any_admission_lookup(mut scope: Scope) {
	scope.run = None;
	assert!(matches!(
		run_for_action(&mut scope, Uuid::from_u128(9)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["run"]);
}
#[rstest]
#[case::owner("owner")]
#[case::audit("audit")]
#[tokio::test]
async fn ownership_errors_keep_their_opaque_identity(mut scope: Scope, #[case] fail: &'static str) {
	scope.fail = Some(fail);
	let Error::Port(error) = allowed(&mut scope, Uuid::from_u128(1), "thread.read")
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}
#[rstest]
#[case::run("run")]
#[case::owner("owner")]
#[case::require("require")]
#[case::provenance("run_visible")]
#[tokio::test]
async fn interaction_errors_keep_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = run_for_interaction(&mut scope, Uuid::from_u128(9))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last().unwrap(), fail);
}

#[test]
fn a_subnode_creator_reaches_its_own_and_the_subnode_threads_but_reads_only_the_latter() {
	let own = Uuid::new_v4();
	let subnode = Uuid::new_v4();
	let other = Uuid::new_v4();
	let context = json!({"thread_id":own,"subnode_read_threads":[subnode]});
	assert!(super::thread_reachable(&context, own));
	assert!(super::thread_reachable(&context, subnode));
	assert!(!super::thread_reachable(&context, other));
	assert!(!super::subnode_read_thread(&context, own));
	assert!(super::subnode_read_thread(&context, subnode));
	assert!(!super::subnode_read_thread(
		&json!({"thread_id":own}),
		subnode
	));
}

fn bundle(policies: Value) -> aidash_domain::policy::PolicyBundle {
	serde_json::from_value(
		json!({"tenant":"tenant","subjects":{"reader":{"kind":"user"}},"policies":policies}),
	)
	.unwrap()
}
#[rstest]
#[tokio::test]
async fn a_node_administrator_reads_every_thread_of_their_node(mut scope: Scope) {
	// Arrange: no policy names Thread 4; the reader administers its Node.
	let thread = Uuid::from_u128(4);
	let administer = json!({"id":"administer","effect":"allow","subjects":{"ids":["reader"]},
		"actions":["node.administer"],"resources":{"kinds":["node"],"ids":["home-node"]}});
	scope.snapshot.bundle = bundle(json!([administer.clone()]));
	// Act + Assert: they read the Thread and its events (#1008, ADR 0140) ...
	assert!(allowed(&mut scope, thread, "thread.read").await.unwrap());
	assert!(allowed(&mut scope, thread, "thread.events").await.unwrap());
	// ... but administering the Node changes nothing else in the Thread.
	assert!(!allowed(&mut scope, thread, "thread.update").await.unwrap());
	// A Run's inherited authority stays confined to its own Thread, and so
	// does a worker's execution grant reading for a Run.
	scope.inherited = true;
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
	scope.inherited = false;
	scope.reads_for_run = true;
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
	scope.reads_for_run = false;
	// Another Node's Thread stays hidden from them.
	scope.node = "other-node".into();
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
	// A member who administers no Node reads nothing they were not given.
	scope.node = "home-node".into();
	scope.snapshot.bundle = bundle(json!([]));
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
	// A policy that denies the Thread by name wins over the Node.
	scope.snapshot.bundle = bundle(json!([administer, {"id":"hide","effect":"deny",
		"subjects":{"any":true},"actions":["thread.read"],
		"resources":{"kinds":["thread"],"ids":[thread]}}]));
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
}

/// A policy that admits one Duty's Thread by `/duty_id`.
fn duty_policy(effect: &str, duty: Uuid) -> Value {
	json!({"id":format!("duty-{effect}"),"effect":effect,"subjects":{"any":true},
		"actions":["thread.read","thread.events"],"resources":{"kinds":["thread"]},
		"condition":{"op":"eq","left":{"source":"resource","path":"/duty_id"},
			"right":{"source":"literal","value":duty}}})
}

#[rstest]
#[tokio::test]
async fn a_duty_thread_carries_its_duty_and_other_threads_carry_none(mut scope: Scope) {
	let (duty_thread, duty) = (Uuid::from_u128(1), Uuid::from_u128(70));
	scope.duties.insert(duty_thread, duty);
	let read = resource(&mut scope, duty_thread).await.unwrap();
	assert_eq!(
		read.attributes,
		json!({"owner":"stored-owner","thread_id":duty_thread,"duty_id":duty,"created_by":"lease-default"})
	);
	// The lease context of a Run in a Duty's Thread never lends that Duty to
	// another Thread the Run reads.
	scope.context = json!({"thread_id":duty_thread,"duty_id":duty});
	let other = resource(&mut scope, Uuid::from_u128(2)).await.unwrap();
	assert_eq!(
		other.attributes,
		json!({"owner":"stored-owner","thread_id":Uuid::from_u128(2)})
	);
	assert_eq!(
		super::attributes(Uuid::from_u128(2), &ThreadOwner::person("someone")),
		json!({"owner":"someone","thread_id":Uuid::from_u128(2)})
	);
}

#[rstest]
#[tokio::test]
async fn a_duty_condition_answers_the_same_on_a_read_a_list_and_events(mut scope: Scope) {
	// Arrange: Threads 1 and 3 are Duty 70's and Duty 71's own Threads; the
	// policy admits Duty 70 only.
	let (wanted, other) = (Uuid::from_u128(70), Uuid::from_u128(71));
	scope.duties.insert(Uuid::from_u128(1), wanted);
	scope.duties.insert(Uuid::from_u128(3), other);
	scope.snapshot.bundle = bundle(json!([duty_policy("allow", wanted)]));
	// A Tenant may save such a policy.
	scope.snapshot.bundle.validate().unwrap();
	// Act: a list, the event stream's Threads and single reads.
	let listed = visible(&mut scope, "thread.read").await.unwrap();
	let streamed = event_threads(&mut scope, None).await.unwrap();
	let mut read = vec![];
	for id in [1, 2, 3, 4].map(Uuid::from_u128) {
		scope.owner = scope
			.rows
			.iter()
			.find(|(row, _)| *row == id)
			.map(|(_, owner)| owner.clone());
		if allowed(&mut scope, id, "thread.read").await.unwrap() {
			read.push(id);
		}
	}
	// Assert: every path admits exactly Duty 70's Thread.
	assert_eq!(listed, vec![Uuid::from_u128(1)]);
	assert_eq!(streamed, vec![Uuid::from_u128(1)]);
	assert_eq!(read, vec![Uuid::from_u128(1)]);
	// Every recorded thread evaluation carried the Thread's own Duty.
	for (input, _) in scope
		.records
		.iter()
		.filter(|(input, _)| input.resource.kind == "thread")
	{
		let id: Uuid = input.resource.id.parse().unwrap();
		assert_eq!(
			input.resource.attributes.get("duty_id"),
			scope.duties.get(&id).map(|duty| json!(duty)).as_ref(),
			"{input:?}"
		);
	}
}

#[rstest]
#[tokio::test]
async fn a_policy_denying_a_duty_thread_stops_its_node_administrator(mut scope: Scope) {
	// Arrange: the reader administers the Node; a policy denies Duty 70's Thread.
	let (thread, duty) = (Uuid::from_u128(4), Uuid::from_u128(70));
	scope.duties.insert(thread, duty);
	let administer = json!({"id":"administer","effect":"allow","subjects":{"ids":["reader"]},
		"actions":["node.administer"],"resources":{"kinds":["node"],"ids":["home-node"]}});
	scope.snapshot.bundle = bundle(json!([administer.clone()]));
	assert!(allowed(&mut scope, thread, "thread.read").await.unwrap());
	// Act + Assert: the Duty-wide deny wins over the Node, on a read and in a list.
	scope.snapshot.bundle = bundle(json!([administer, duty_policy("deny", duty)]));
	assert!(!allowed(&mut scope, thread, "thread.read").await.unwrap());
	scope.rows = vec![(thread, "stored-owner".into())];
	assert!(visible(&mut scope, "thread.read").await.unwrap().is_empty());
}

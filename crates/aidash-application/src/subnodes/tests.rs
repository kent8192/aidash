use super::*;
use crate::{authorization::Snapshot, ports::authorization::lease::AuthorizationLease};
use aidash_domain::{
	RunControl, RunPhase,
	identity::{tenancy, tenant_administration as admin},
	policy::{Decision, Effect, Policy, ResourceSelector, SubjectSelector},
	subnodes::{Member, State, SubnodeId, SubnodeLimits},
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		home_node: "aidash://local/brave-otter-k3m9q2xa".into(),
		agent_id: "creator".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		control_revision: 0,
		control_source: Default::default(),
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
fn agent() -> EntityRef {
	EntityRef {
		id: "leader".into(),
		version: "1".into(),
	}
}
fn input() -> Create {
	Create {
		leader: agent(),
		members: vec![agent()],
		allowance: Allowance::default(),
		title: None,
	}
}
struct Scope {
	snapshot: Snapshot,
	subjects: Vec<String>,
	environment: Value,
	decisions: Vec<(Evaluation, Decision)>,
	inserted: usize,
	member_required: Allowance,
	remote: bool,
	nested: bool,
	active: usize,
}
fn scope() -> Scope {
	let run = run();
	let creator = aidash_domain::agent_subject(&run.home_node, &run.agent_id, &run.agent_version);
	let mut bundle = tenancy::initial_policy("tenant", true).unwrap();
	admin::add_agent_subject(
		&mut bundle,
		&creator,
		&[tenancy::MEMBERS.to_string()].into(),
	)
	.unwrap();
	Scope {
		snapshot: Snapshot {
			revision: 1,
			bundle,
		},
		subjects: vec![tenancy::OWNER.into(), creator],
		environment: json!({}),
		decisions: vec![],
		inserted: 0,
		member_required: Allowance::default(),
		remote: false,
		nested: false,
		active: 0,
	}
}
impl Scope {
	fn grant(&mut self) {
		self.snapshot.bundle.policies.push(Policy {
			id: "grant-subnode".into(),
			effect: Effect::Allow,
			subjects: SubjectSelector {
				ids: [self.subjects[1].clone()].into(),
				..Default::default()
			},
			actions: [admin::SUBNODE_CREATE.into()].into(),
			resources: ResourceSelector {
				kinds: ["node".into()].into(),
				ids: Default::default(),
			},
			condition: None,
		});
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
		self.decisions.extend_from_slice(records);
		Ok(())
	}
}
#[async_trait]
impl CreateScope for Scope {
	fn execution_node(&self) -> &str {
		"aidash://local/brave-otter-k3m9q2xa"
	}
	async fn node_resource(
		&mut self,
		run: &RunMetadata,
	) -> Result<aidash_domain::policy::Resource> {
		Ok(aidash_domain::policy::Resource {
			tenant: "tenant".into(),
			kind: "node".into(),
			id: run.node.to_string(),
			attributes: json!({}),
		})
	}
	async fn workspace_is_subnode(&mut self, _: Uuid) -> Result<bool> {
		Ok(self.nested)
	}
	async fn remote_grant_started_run(&mut self, _: Uuid) -> Result<bool> {
		Ok(self.remote)
	}
	async fn replay(&mut self, _: &RunMetadata, _: &str, _: &Create) -> Result<Option<Subnode>> {
		Ok(None)
	}
	async fn limits(&mut self) -> Result<SubnodeLimits> {
		Ok(SubnodeLimits::default())
	}
	async fn active_count(&mut self, _: Uuid) -> Result<usize> {
		Ok(self.active)
	}
	async fn creator_authority(&mut self, _: &RunMetadata) -> Result<Allowance> {
		Ok(Allowance::default())
	}
	async fn approved_member_authority(&mut self, _: &EntityRef) -> Result<Allowance> {
		Ok(self.member_required.clone())
	}
	async fn insert(&mut self, run: &RunMetadata, _: &str, input: &Create) -> Result<Subnode> {
		self.inserted += 1;
		let now = Utc::now();
		Ok(Subnode {
			id: SubnodeId(Uuid::from_u128(4)),
			tenant: "tenant".into(),
			node: run.home_node.clone(),
			creator_run_id: run.id,
			creator_task_id: run.task_id,
			creator_agent: EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
			leader: input.leader.clone(),
			members: input
				.members
				.iter()
				.map(|a| Member {
					agent_id: a.id.clone(),
					agent_version: a.version.clone(),
					added_by_run_id: run.id,
					added_at: now,
				})
				.collect(),
			allowance: input.allowance.clone(),
			workspace_id: Uuid::from_u128(5),
			title: input.title.clone(),
			state: State::Active,
			created_at: now,
			ended_at: None,
			ended_reason: None,
		})
	}
}

#[tokio::test]
async fn creator_agent_needs_an_explicit_grant_but_the_root_does_not() {
	let mut scope = scope();
	assert!(matches!(
		create(&mut scope, &run(), "key", &input()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.inserted, 0);
	scope.grant();
	let created = create(&mut scope, &run(), "key", &input()).await.unwrap();
	assert_eq!(created.creator_run_id, run().id);
	assert_eq!(scope.inserted, 1);
	assert_eq!(scope.decisions.len(), 2);
	assert!(
		scope
			.decisions
			.iter()
			.all(|(evaluation, _)| evaluation.subject == scope.subjects[1])
	);
}

#[tokio::test]
async fn creation_refuses_nested_remote_over_limit_and_uncovered_member() {
	let mut scope = scope();
	scope.grant();
	scope.nested = true;
	assert!(matches!(
		create(&mut scope, &run(), "key", &input()).await,
		Err(Error::Refused(_))
	));
	scope.nested = false;
	scope.remote = true;
	assert!(matches!(
		create(&mut scope, &run(), "key", &input()).await,
		Err(Error::Refused(_))
	));
	scope.remote = false;
	scope.active = 3;
	assert!(matches!(
		create(&mut scope, &run(), "key", &input()).await,
		Err(Error::Refused(_))
	));
	scope.active = 0;
	scope.member_required.tools.insert("private@1".into());
	assert!(
		matches!(create(&mut scope, &run(), "key", &input()).await, Err(Error::Refused(message)) if message.contains("private@1"))
	);
	assert_eq!(scope.inserted, 0);
	scope.member_required = Allowance::default();
	let mut request = input();
	request.members.push(EntityRef {
		id: run().agent_id,
		version: "2".into(),
	});
	assert!(
		matches!(create(&mut scope, &run(), "key", &request).await, Err(Error::Refused(message)) if message.contains("creating agent"))
	);
}

struct Management {
	subnode: Subnode,
	member_required: Allowance,
	has_work: bool,
	effects: Vec<&'static str>,
}
impl Management {
	fn new() -> Self {
		let run = run();
		let now = Utc::now();
		Self {
			subnode: Subnode {
				id: SubnodeId(Uuid::from_u128(4)),
				tenant: "tenant".into(),
				node: run.home_node.clone(),
				creator_run_id: run.id,
				creator_task_id: run.task_id,
				creator_agent: EntityRef {
					id: run.agent_id,
					version: run.agent_version,
				},
				leader: agent(),
				members: vec![Member {
					agent_id: "leader".into(),
					agent_version: "1".into(),
					added_by_run_id: run.id,
					added_at: now,
				}],
				allowance: Allowance::default(),
				workspace_id: Uuid::from_u128(5),
				title: None,
				state: State::Active,
				created_at: now,
				ended_at: None,
				ended_reason: None,
			},
			member_required: Allowance::default(),
			has_work: false,
			effects: vec![],
		}
	}
}
#[async_trait]
impl crate::ports::subnodes::ManageScope for Management {
	async fn lock(&mut self, id: SubnodeId) -> Result<Option<Subnode>> {
		Ok((id == self.subnode.id).then(|| self.subnode.clone()))
	}
	async fn limits(&mut self, _: &str) -> Result<SubnodeLimits> {
		Ok(SubnodeLimits::default())
	}
	async fn creator_authority(&mut self, subnode: &Subnode) -> Result<Allowance> {
		Ok(subnode.allowance.clone())
	}
	async fn approved_member_authority(&mut self, _: &Subnode, _: &EntityRef) -> Result<Allowance> {
		Ok(self.member_required.clone())
	}
	async fn member_has_active_work(&mut self, _: &Subnode, _: &EntityRef) -> Result<bool> {
		Ok(self.has_work)
	}
	async fn add_member(&mut self, _: &Subnode, _: &EntityRef, _: Uuid) -> Result<()> {
		self.effects.push("add");
		Ok(())
	}
	async fn remove_member(&mut self, _: &Subnode, _: &EntityRef) -> Result<()> {
		self.effects.push("remove");
		Ok(())
	}
	async fn task(
		&mut self,
		_: &Subnode,
		_: &RunMetadata,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<Uuid> {
		self.effects.push("task");
		Ok(Uuid::from_u128(6))
	}
	async fn dissolve(&mut self, _: &Subnode, _: subnodes::EndReason) -> Result<()> {
		self.effects.push("dissolve");
		Ok(())
	}
	async fn message_creator(
		&mut self,
		_: &Subnode,
		_: &RunMetadata,
		_: &str,
		_: &str,
	) -> Result<()> {
		self.effects.push("message");
		Ok(())
	}
}
#[tokio::test]
async fn management_requires_creator_or_member_and_preserves_leader() {
	let mut scope = Management::new();
	let id = scope.subnode.id;
	let mut member = run();
	member.id = Uuid::from_u128(9);
	member.workspace_id = scope.subnode.workspace_id;
	member.agent_id = "leader".into();
	let mut outsider = member.clone();
	outsider.agent_id = "outsider".into();
	assert!(matches!(
		task(&mut scope, &member, id, "key", "title", "request").await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		message(&mut scope, &outsider, id, "key", "text").await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		remove_member(&mut scope, &run(), id, &agent()).await,
		Err(Error::Refused(_))
	));
	assert!(scope.effects.is_empty());
	task(&mut scope, &run(), id, "key", "title", "request")
		.await
		.unwrap();
	message(&mut scope, &member, id, "key", "text")
		.await
		.unwrap();
	assert_eq!(scope.effects, ["task", "message"]);
}
#[tokio::test]
async fn addition_refuses_an_uncovered_binding_and_dissolution_requires_creator() {
	let mut scope = Management::new();
	let id = scope.subnode.id;
	let candidate = EntityRef {
		id: "worker".into(),
		version: "1".into(),
	};
	scope.member_required.tools.insert("outside@1".into());
	assert!(
		matches!(add_member(&mut scope, &run(), id, &candidate).await, Err(Error::Refused(message)) if message.contains("outside@1"))
	);
	let mut outsider = run();
	outsider.id = Uuid::from_u128(99);
	assert!(matches!(
		dissolve(&mut scope, &outsider, id).await,
		Err(Error::Forbidden)
	));
	assert!(scope.effects.is_empty());
	dissolve(&mut scope, &run(), id).await.unwrap();
	assert_eq!(scope.effects, ["dissolve"]);
}

#[derive(Default)]
struct Ending {
	marks: Vec<subnodes::EndReason>,
	controls: Vec<(Uuid, aidash_domain::RunControlAction)>,
}
#[async_trait]
impl crate::ports::subnodes::EndScope for Ending {
	async fn active_subnodes(&mut self, _: Uuid) -> Result<Vec<(SubnodeId, Uuid)>> {
		Ok(vec![(SubnodeId(Uuid::from_u128(4)), Uuid::from_u128(5))])
	}
	async fn mark_ended(
		&mut self,
		_: SubnodeId,
		_: Uuid,
		reason: subnodes::EndReason,
	) -> Result<()> {
		self.marks.push(reason);
		Ok(())
	}
	async fn active_runs(&mut self, _: Uuid) -> Result<Vec<Uuid>> {
		Ok(vec![Uuid::from_u128(8), Uuid::from_u128(9)])
	}
	async fn control_run(
		&mut self,
		run: Uuid,
		action: aidash_domain::RunControlAction,
		_: aidash_domain::ControlSource,
	) -> Result<()> {
		self.controls.push((run, action));
		Ok(())
	}
}
#[tokio::test]
async fn creator_pause_preserves_subnode_and_terminal_end_cancels_members() {
	use aidash_domain::{ControlSource, RunControlAction};
	let mut scope = Ending::default();
	control_for_creator(
		&mut scope,
		run().id,
		RunControlAction::Pause,
		ControlSource::DeploymentAdministrator,
		None,
	)
	.await
	.unwrap();
	assert!(scope.marks.is_empty());
	assert_eq!(
		scope.controls,
		[
			(Uuid::from_u128(8), RunControlAction::Pause),
			(Uuid::from_u128(9), RunControlAction::Pause)
		]
	);
	scope.controls.clear();
	control_for_creator(
		&mut scope,
		run().id,
		RunControlAction::Cancel,
		ControlSource::System,
		Some(subnodes::EndReason::CreatorFailed),
	)
	.await
	.unwrap();
	assert_eq!(scope.marks, [subnodes::EndReason::CreatorFailed]);
	assert_eq!(
		scope.controls,
		[
			(Uuid::from_u128(8), RunControlAction::Cancel),
			(Uuid::from_u128(9), RunControlAction::Cancel)
		]
	);
}

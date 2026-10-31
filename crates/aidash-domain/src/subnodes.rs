//! Pure authority and membership rules for Run-scoped Subnodes.
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::registry::EntityRef;

/// Coordination tools available independently of the allowance.
pub const COORDINATION_TOOLS: &[&str] = &[
	"task_create",
	"task_delegate",
	"task_assign",
	"agent_discover",
	"workspace_message",
	"subnode_message",
	"subnode_add_member",
	"subnode_remove_member",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubnodeId(pub Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
	pub agent_id: String,
	pub agent_version: String,
	pub added_by_run_id: Uuid,
	pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
	Active,
	Dissolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
	Dissolved,
	CreatorCompleted,
	CreatorFailed,
	CreatorCancelled,
	SafetyStop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subnode {
	pub id: SubnodeId,
	pub tenant: String,
	pub node: String,
	pub creator_run_id: Uuid,
	pub creator_task_id: Uuid,
	pub creator_agent: EntityRef,
	pub leader: EntityRef,
	pub members: Vec<Member>,
	pub allowance: Allowance,
	pub workspace_id: Uuid,
	pub title: Option<String>,
	pub state: State,
	pub created_at: DateTime<Utc>,
	pub ended_at: Option<DateTime<Utc>>,
	pub ended_reason: Option<EndReason>,
}

impl Member {
	fn is_agent(&self, agent: &EntityRef) -> bool {
		self.agent_id == agent.id && self.agent_version == agent.version
	}
}

impl Subnode {
	pub fn allows_delegation(&self, target: &EntityRef) -> bool {
		self.state == State::Active && self.members.iter().any(|member| member.is_agent(target))
	}

	pub fn check_removal(&self, agent: &EntityRef, has_active_run: bool) -> Result<(), Refusal> {
		if !self.members.iter().any(|member| member.is_agent(agent)) {
			return Err(Refusal::NotMember);
		}
		if &self.leader == agent {
			return Err(Refusal::LeaderRemoval);
		}
		if has_active_run {
			return Err(Refusal::MemberHasWork);
		}
		Ok(())
	}
}

/// Check the intended member set before persistence, including the one-version-per-agent rule.
pub fn check_member_set(
	leader: &EntityRef,
	members: &[Member],
	limits: SubnodeLimits,
) -> Result<(), Refusal> {
	limits.check_members(members.len())?;
	let mut ids = BTreeSet::new();
	for member in members {
		if !ids.insert(&member.agent_id) {
			return Err(Refusal::DuplicateMember(member.agent_id.clone()));
		}
	}
	check_leader(leader, members)
}

pub fn check_leader(leader: &EntityRef, members: &[Member]) -> Result<(), Refusal> {
	if members.iter().any(|member| member.is_agent(leader)) {
		Ok(())
	} else {
		Err(Refusal::LeaderNotMember)
	}
}

pub fn check_can_create(is_member: bool) -> Result<(), Refusal> {
	if is_member {
		Err(Refusal::NestedSubnode)
	} else {
		Ok(())
	}
}

/// The tools and Tenant Secrets the creator permits members to use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Allowance {
	pub tools: BTreeSet<String>,
	pub secrets: BTreeSet<String>,
}

/// A correctable refusal, naming the first item outside the permitted set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
	#[error("coordination tool must not appear in the allowance: {0}")]
	CoordinationToolInAllowance(String),
	#[error("tool is outside the creator's authority: {0}")]
	CreatorTool(String),
	#[error("secret is outside the creator's authority: {0}")]
	CreatorSecret(String),
	#[error("member needs a tool outside the allowance: {0}")]
	MemberTool(String),
	#[error("member needs a secret outside the allowance: {0}")]
	MemberSecret(String),
	#[error("Subnode member limit reached")]
	MemberLimit,
	#[error("active Subnode limit reached")]
	SubnodeLimit,
	#[error("agent already belongs to the Subnode: {0}")]
	DuplicateMember(String),
	#[error("the creating agent cannot be a Subnode member")]
	CreatorMember,
	#[error("leader must be a member")]
	LeaderNotMember,
	#[error("leader cannot be removed")]
	LeaderRemoval,
	#[error("member has unfinished work")]
	MemberHasWork,
	#[error("agent is not a member")]
	NotMember,
	#[error("a Subnode member cannot create another Subnode")]
	NestedSubnode,
	#[error("remote Runs are not permitted to create Subnodes")]
	RemoteCreationDisabled,
	#[error("invalid Subnode limits")]
	InvalidLimits,
}

/// Reject an allowance that exceeds what the creating Run may use.
pub fn allowance_within(creator: &Allowance, requested: &Allowance) -> Result<(), Refusal> {
	if let Some(item) = requested
		.tools
		.iter()
		.find(|tool| COORDINATION_TOOLS.contains(&tool.as_str()))
	{
		return Err(Refusal::CoordinationToolInAllowance(item.clone()));
	}
	if let Some(item) = requested.tools.difference(&creator.tools).next() {
		return Err(Refusal::CreatorTool(item.clone()));
	}
	if let Some(item) = requested.secrets.difference(&creator.secrets).next() {
		return Err(Refusal::CreatorSecret(item.clone()));
	}
	Ok(())
}

/// Reject a member whose binding requires authority outside the allowance.
pub fn member_fits(required: &Allowance, allowance: &Allowance) -> Result<(), Refusal> {
	if let Some(item) = required
		.tools
		.difference(&allowance.tools)
		.find(|tool| !COORDINATION_TOOLS.contains(&tool.as_str()))
	{
		return Err(Refusal::MemberTool(item.clone()));
	}
	if let Some(item) = required.secrets.difference(&allowance.secrets).next() {
		return Err(Refusal::MemberSecret(item.clone()));
	}
	Ok(())
}

/// Tenant settings; callers must validate settings before storing or using them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubnodeLimits {
	pub members_per_subnode: u32,
	pub subnodes_per_run: u32,
	pub remote_runs_may_create: bool,
}

impl Default for SubnodeLimits {
	fn default() -> Self {
		Self {
			members_per_subnode: 5,
			subnodes_per_run: 3,
			remote_runs_may_create: false,
		}
	}
}

impl SubnodeLimits {
	pub fn validate(self) -> Result<(), Refusal> {
		if !(1..=20).contains(&self.members_per_subnode)
			|| !(1..=10).contains(&self.subnodes_per_run)
		{
			return Err(Refusal::InvalidLimits);
		}
		Ok(())
	}

	pub fn check_remote(self, remote: bool) -> Result<(), Refusal> {
		self.validate()?;
		if remote && !self.remote_runs_may_create {
			return Err(Refusal::RemoteCreationDisabled);
		}
		Ok(())
	}

	pub fn check_members(self, count: usize) -> Result<(), Refusal> {
		self.validate()?;
		if count > self.members_per_subnode as usize {
			return Err(Refusal::MemberLimit);
		}
		Ok(())
	}

	pub fn check_create(self, active: usize) -> Result<(), Refusal> {
		self.validate()?;
		if active >= self.subnodes_per_run as usize {
			return Err(Refusal::SubnodeLimit);
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn allowance(tools: &[&str], secrets: &[&str]) -> Allowance {
		Allowance {
			tools: tools.iter().map(|s| (*s).into()).collect(),
			secrets: secrets.iter().map(|s| (*s).into()).collect(),
		}
	}

	#[test]
	fn authority_is_narrowed_at_both_boundaries() {
		let creator = allowance(&["a", "b"], &["s"]);
		let allowed = allowance(&["a"], &[]);
		assert_eq!(allowance_within(&creator, &allowed), Ok(()));
		assert_eq!(
			allowance_within(&creator, &allowance(&["c"], &[])),
			Err(Refusal::CreatorTool("c".into()))
		);
		assert_eq!(
			allowance_within(&creator, &allowance(&[], &["t"])),
			Err(Refusal::CreatorSecret("t".into()))
		);
		assert_eq!(
			member_fits(&allowance(&["b"], &[]), &allowed),
			Err(Refusal::MemberTool("b".into()))
		);
		assert_eq!(
			member_fits(&allowance(&[], &["s"]), &allowed),
			Err(Refusal::MemberSecret("s".into()))
		);
		assert_eq!(member_fits(&allowed, &allowed), Ok(()));
		assert_eq!(
			allowance_within(&creator, &allowance(&["task_create"], &[])),
			Err(Refusal::CoordinationToolInAllowance("task_create".into()))
		);
	}

	#[test]
	fn limits_apply_to_resulting_members_and_existing_subnodes() {
		let limits = SubnodeLimits::default();
		assert_eq!(limits.check_remote(false), Ok(()));
		assert_eq!(
			limits.check_remote(true),
			Err(Refusal::RemoteCreationDisabled)
		);
		assert_eq!(
			SubnodeLimits {
				remote_runs_may_create: true,
				..limits
			}
			.check_remote(true),
			Ok(())
		);
		assert_eq!(limits.check_members(5), Ok(()));
		assert_eq!(limits.check_members(6), Err(Refusal::MemberLimit));
		assert_eq!(limits.check_create(2), Ok(()));
		assert_eq!(limits.check_create(3), Err(Refusal::SubnodeLimit));
		assert_eq!(
			SubnodeLimits {
				members_per_subnode: 0,
				..limits
			}
			.validate(),
			Err(Refusal::InvalidLimits)
		);
		assert_eq!(
			SubnodeLimits {
				subnodes_per_run: 11,
				..limits
			}
			.validate(),
			Err(Refusal::InvalidLimits)
		);
	}
	#[test]
	fn membership_controls_delegation_and_removal() {
		let leader = EntityRef {
			id: "leader".into(),
			version: "1".into(),
		};
		let worker = EntityRef {
			id: "worker".into(),
			version: "2".into(),
		};
		let member = |agent: EntityRef| Member {
			agent_id: agent.id,
			agent_version: agent.version,
			added_by_run_id: Uuid::nil(),
			added_at: Utc::now(),
		};
		let members = vec![member(leader.clone()), member(worker.clone())];
		assert_eq!(
			check_member_set(&worker, &members, SubnodeLimits::default()),
			Ok(())
		);
		let duplicate = vec![member(leader.clone()), member(leader.clone())];
		assert_eq!(
			check_member_set(&leader, &duplicate, SubnodeLimits::default()),
			Err(Refusal::DuplicateMember("leader".into()))
		);
		let outsider = EntityRef {
			id: "worker".into(),
			version: "3".into(),
		};
		assert_eq!(
			check_leader(&outsider, &members),
			Err(Refusal::LeaderNotMember)
		);
		let mut subnode = Subnode {
			id: SubnodeId(Uuid::nil()),
			tenant: "tenant".into(),
			node: "node".into(),
			creator_run_id: Uuid::nil(),
			creator_task_id: Uuid::nil(),
			creator_agent: outsider.clone(),
			leader: leader.clone(),
			members,
			allowance: Allowance::default(),
			workspace_id: Uuid::nil(),
			title: None,
			state: State::Active,
			created_at: Utc::now(),
			ended_at: None,
			ended_reason: None,
		};
		assert!(subnode.allows_delegation(&worker));
		assert!(!subnode.allows_delegation(&outsider));
		assert_eq!(
			subnode.check_removal(&leader, false),
			Err(Refusal::LeaderRemoval)
		);
		assert_eq!(
			subnode.check_removal(&worker, true),
			Err(Refusal::MemberHasWork)
		);
		assert_eq!(subnode.check_removal(&worker, false), Ok(()));
		assert_eq!(
			subnode.check_removal(&outsider, false),
			Err(Refusal::NotMember)
		);
		subnode.state = State::Dissolved;
		assert!(!subnode.allows_delegation(&worker));
		assert_eq!(check_can_create(true), Err(Refusal::NestedSubnode));
		assert_eq!(check_can_create(false), Ok(()));
	}

	#[test]
	fn coordination_tools_do_not_need_explicit_allowance() {
		assert_eq!(
			member_fits(
				&allowance(&["task_delegate", "subnode_message"], &[]),
				&Allowance::default()
			),
			Ok(())
		);
	}
}

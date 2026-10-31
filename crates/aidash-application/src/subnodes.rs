//! Authorized use cases for Run-scoped Subnodes.
use crate::{Error, Result, ports::subnodes::CreateScope};
use aidash_domain::{
	RunMetadata,
	policy::Evaluation,
	registry::EntityRef,
	subnodes::{self, Allowance, Subnode},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
	pub leader: EntityRef,
	pub members: Vec<EntityRef>,
	pub allowance: Allowance,
	pub title: Option<String>,
}

fn refused(error: subnodes::Refusal) -> Error {
	Error::Refused(error.to_string())
}

/// Create under one transaction. The single-subject policy check deliberately
/// does not intersect the creator's chain; admission still checks the chain.
pub async fn create(
	scope: &mut dyn CreateScope,
	run: &RunMetadata,
	key: &str,
	input: &Create,
) -> Result<Subnode> {
	let creator =
		aidash_domain::agent_subject(scope.execution_node(), &run.agent_id, &run.agent_version);
	if scope.subjects().last() != Some(&creator) {
		return Err(Error::Forbidden);
	}
	let resource = scope.node_resource(run).await?;
	let evaluation = Evaluation {
		subject: creator,
		action: aidash_domain::identity::tenant_administration::SUBNODE_CREATE.into(),
		resource,
		environment: scope.environment().clone(),
	};
	let mut decision = scope.snapshot().bundle.evaluate(&evaluation);
	decision.revision = scope.snapshot().revision;
	let allowed = decision.allowed;
	scope.record(&[(evaluation, decision)]).await?;
	if !allowed {
		return Err(Error::Forbidden);
	}
	subnodes::check_can_create(scope.workspace_is_subnode(run.workspace_id).await?)
		.map_err(refused)?;
	if let Some(existing) = scope.replay(run, key, input).await? {
		return Ok(existing);
	}
	let limits = scope.limits().await?;
	limits
		.check_remote(scope.remote_grant_started_run(run.id).await?)
		.map_err(refused)?;
	limits
		.check_create(scope.active_count(run.id).await?)
		.map_err(refused)?;
	limits.check_members(input.members.len()).map_err(refused)?;
	if !input.members.contains(&input.leader) {
		return Err(refused(subnodes::Refusal::LeaderNotMember));
	}
	let mut member_ids = BTreeSet::new();
	for member in &input.members {
		if member.id == run.agent_id {
			return Err(refused(subnodes::Refusal::CreatorMember));
		}
		if !member_ids.insert(&member.id) {
			return Err(refused(subnodes::Refusal::DuplicateMember(
				member.id.clone(),
			)));
		}
	}
	let creator_authority = scope.creator_authority(run).await?;
	subnodes::allowance_within(&creator_authority, &input.allowance).map_err(refused)?;
	for member in &input.members {
		let required = scope.approved_member_authority(member).await?;
		subnodes::member_fits(&required, &input.allowance).map_err(refused)?;
	}
	scope.insert(run, key, input).await
}

#[cfg(test)]
mod tests;

async fn active(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	id: subnodes::SubnodeId,
) -> Result<Subnode> {
	let subnode = scope
		.lock(id)
		.await?
		.ok_or_else(|| Error::NotFound("Subnode not found".into()))?;
	if subnode.state != subnodes::State::Active {
		return Err(Error::Refused("Subnode has ended".into()));
	}
	Ok(subnode)
}

fn creator(subnode: &Subnode, run: &RunMetadata) -> Result<()> {
	if subnode.creator_run_id == run.id {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

fn member(subnode: &Subnode, run: &RunMetadata) -> bool {
	subnode.workspace_id == run.workspace_id
		&& subnode.members.iter().any(|member| {
			member.agent_id == run.agent_id && member.agent_version == run.agent_version
		})
}

fn participant(subnode: &Subnode, run: &RunMetadata) -> Result<()> {
	if subnode.creator_run_id == run.id || member(subnode, run) {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

pub async fn add_member(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
	agent: &EntityRef,
) -> Result<()> {
	let subnode = active(scope, id).await?;
	participant(&subnode, run)?;
	if subnode.creator_agent.id == agent.id {
		return Err(refused(subnodes::Refusal::CreatorMember));
	}
	if subnode
		.members
		.iter()
		.any(|member| member.agent_id == agent.id)
	{
		return Err(refused(subnodes::Refusal::DuplicateMember(
			agent.id.clone(),
		)));
	}
	scope
		.limits(&subnode.tenant)
		.await?
		.check_members(subnode.members.len() + 1)
		.map_err(refused)?;
	let creator_authority = scope.creator_authority(&subnode).await?;
	subnodes::allowance_within(&creator_authority, &subnode.allowance).map_err(refused)?;
	let required = scope.approved_member_authority(&subnode, agent).await?;
	subnodes::member_fits(&required, &subnode.allowance).map_err(refused)?;
	scope.add_member(&subnode, agent, run.id).await
}

pub async fn remove_member(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
	agent: &EntityRef,
) -> Result<()> {
	let subnode = active(scope, id).await?;
	participant(&subnode, run)?;
	let has_work = scope.member_has_active_work(&subnode, agent).await?;
	subnode.check_removal(agent, has_work).map_err(refused)?;
	scope.remove_member(&subnode, agent).await
}

pub async fn task(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
	key: &str,
	title: &str,
	request: &str,
) -> Result<uuid::Uuid> {
	let subnode = active(scope, id).await?;
	creator(&subnode, run)?;
	aidash_domain::nonempty(title, "title")?;
	aidash_domain::nonempty(request, "request")?;
	scope.task(&subnode, run, key, title, request).await
}

pub async fn dissolve(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
) -> Result<()> {
	let subnode = active(scope, id).await?;
	creator(&subnode, run)?;
	scope
		.dissolve(&subnode, subnodes::EndReason::Dissolved)
		.await
}

pub async fn message(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
	key: &str,
	content: &str,
) -> Result<()> {
	let subnode = active(scope, id).await?;
	if !member(&subnode, run) {
		return Err(Error::Forbidden);
	}
	aidash_domain::nonempty(content, "content")?;
	scope.message_creator(&subnode, run, key, content).await
}

/// Propagate a creator control while retaining its Run lock. A pause keeps the
/// Subnode active; a cancel or terminal transition ends it and cancels members.
pub async fn control_for_creator(
	scope: &mut dyn crate::ports::subnodes::EndScope,
	creator: uuid::Uuid,
	action: aidash_domain::RunControlAction,
	source: aidash_domain::ControlSource,
	reason: Option<subnodes::EndReason>,
) -> Result<()> {
	if reason.is_some() && action != aidash_domain::RunControlAction::Cancel {
		return Err(Error::Invalid(
			"ending a Subnode requires cancellation".into(),
		));
	}
	if reason.is_none() && action != aidash_domain::RunControlAction::Pause {
		return Err(Error::Invalid(
			"a nonterminal Subnode control must pause".into(),
		));
	}
	for (id, workspace) in scope.active_subnodes(creator).await? {
		if let Some(reason) = reason {
			scope.mark_ended(id, workspace, reason).await?;
		}
		for run in scope.active_runs(workspace).await? {
			scope.control_run(run, action, source).await?;
		}
	}
	Ok(())
}

/// Arguments after the built-in tool's schema and agent references are parsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Command {
	Task {
		id: subnodes::SubnodeId,
		title: String,
		request: String,
	},
	Add {
		id: subnodes::SubnodeId,
		agent: EntityRef,
	},
	Remove {
		id: subnodes::SubnodeId,
		agent: EntityRef,
	},
	Dissolve {
		id: subnodes::SubnodeId,
	},
	Message {
		content: String,
	},
}

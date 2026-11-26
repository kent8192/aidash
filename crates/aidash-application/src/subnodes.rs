//! Authorized use cases for Run-scoped Subnodes.
use crate::{
	Error, Result,
	ports::subnodes::{CreateScope, EndScope},
};
use aidash_domain::{
	ControlSource, RunControlAction, RunMetadata,
	policy::Evaluation,
	registry::EntityRef,
	subnodes::{self, Allowance, Subnode},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use uuid::Uuid;

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
		// This check evaluates the policy itself rather than through `Access::require`.
		crate::authorization::refusal_note::set(crate::authorization::refusal_note::Note::new(
			aidash_domain::tool::RefusalCheck::Policy,
			Some(aidash_domain::identity::tenant_administration::SUBNODE_CREATE),
		));
		return Err(Error::Forbidden);
	}
	// Subnodes nest, and neither their number nor their members are capped
	// (#1012, ADR 0143). A Run a Remote Grant started needs no setting of its
	// own either: `subnode_create` in both Federation tool lists is what lets
	// it reach this point (ADR 0139).
	if let Some(existing) = scope.replay(run, key, input).await? {
		return Ok(existing);
	}
	if !input.members.contains(&input.leader) {
		return Err(refused(subnodes::Refusal::LeaderNotMember));
	}
	let mut member_ids = BTreeSet::new();
	for member in &input.members {
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

/// Refuse a member that needs a tool the creating Run may not use. A Run a Remote Grant started
/// uses a tool only when both Nodes' Federation lists allow it (ADR 0139), so its Subnode members,
/// who use at most what it may use (ADR 0110), need every tool they bind to pass the same
/// decision. `keys` are the list keys of the member's tool Bindings. The coordination tools act
/// only inside the Subnode and are not listed. A tool a side merely lacks refuses the member as
/// well: nobody is asked on a member's behalf, the creating Run has to call the tool first.
pub fn within_federation_lists(
	lists: &aidash_domain::federation::tool_lists::OwnedRemoteToolLists,
	keys: &BTreeSet<String>,
) -> Result<()> {
	use aidash_domain::{
		federation::tool_lists::ToolVerdict,
		tool::{ToolReach, builtin_reach},
	};
	for key in keys {
		if subnodes::COORDINATION_TOOLS.contains(&key.as_str()) {
			continue;
		}
		// Only a Registry tool, or a descriptor of another provider, has no builtin name.
		let reach = builtin_reach(key).unwrap_or(ToolReach::ExternalServices);
		if !matches!(lists.lists().decide(reach, key), ToolVerdict::Usable) {
			return Err(Error::Refused(format!(
				"member needs a tool the creating Run may not use under the Federation tool lists: {key}"
			)));
		}
	}
	Ok(())
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
	subnode.thread_id == run.thread_id
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
	if subnode
		.members
		.iter()
		.any(|member| member.agent_id == agent.id)
	{
		return Err(refused(subnodes::Refusal::DuplicateMember(
			agent.id.clone(),
		)));
	}
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
	journal_range: Option<subnodes::JournalRange>,
	task_model: Option<aidash_domain::TaskModel>,
) -> Result<uuid::Uuid> {
	let subnode = active(scope, id).await?;
	creator(&subnode, run)?;
	aidash_domain::nonempty(title, "title")?;
	aidash_domain::nonempty(request, "request")?;
	scope
		.task(
			&subnode,
			run,
			key,
			title,
			request,
			journal_range,
			task_model,
		)
		.await
}

/// Deletes a Subnode (`delete_subnode`). The creating Agent may do so from any
/// Run of the same flow, not only the creating Run (#1012 decision 46): the
/// Subnode outlives that Run. Another flow does not, even of the same person;
/// that flow's root person and Tenant Administrators delete it from a browser
/// session. Its records stay readable.
pub async fn dissolve(
	scope: &mut dyn crate::ports::subnodes::ManageScope,
	run: &RunMetadata,
	id: subnodes::SubnodeId,
) -> Result<()> {
	let subnode = active(scope, id).await?;
	if creator(&subnode, run).is_err() {
		let same_agent = subnode.creator_agent.id == run.agent_id
			&& subnode.creator_agent.version == run.agent_version;
		let roots = scope
			.flow_roots(&[run.task_id, subnode.creator_task_id])
			.await?;
		let flow = roots.get(&run.task_id);
		if !same_agent || flow.is_none() || flow != roots.get(&subnode.creator_task_id) {
			return Err(Error::Forbidden);
		}
	}
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

#[derive(Default)]
struct Descendants {
	subnodes: Vec<(subnodes::SubnodeId, Uuid)>,
	runs: Vec<Uuid>,
}

/// Lock from the creator down, with every Subnode held before its member
/// Runs. Writers and workers take those locks before the event lock, so the
/// whole walk must finish before a control or dissolution appends an event.
async fn lock_descendants(
	scope: &mut dyn EndScope,
	creator: Uuid,
	resuming: bool,
) -> Result<Descendants> {
	let mut descendants = Descendants::default();
	let mut pending = VecDeque::from([creator]);
	let mut seen = BTreeSet::from([creator]);
	while let Some(creator) = pending.pop_front() {
		for (id, thread) in scope.active_subnodes(creator).await? {
			descendants.subnodes.push((id, thread));
			for (run, controllable) in scope.member_runs(thread, resuming).await? {
				if seen.insert(run) {
					pending.push_back(run);
					if controllable {
						descendants.runs.push(run);
					}
				}
			}
		}
	}
	Ok(descendants)
}

/// A caller that appends an event before cascading locks descendants first.
pub async fn lock_end(scope: &mut dyn EndScope, creator: Uuid) -> Result<()> {
	lock_descendants(scope, creator, false).await?;
	Ok(())
}

/// Propagate a creator control while retaining its Run lock. A pause or resume
/// keeps the Subnodes active. Cancellation ends them and cancels their members.
pub async fn control_for_creator(
	scope: &mut dyn EndScope,
	creator: Uuid,
	action: RunControlAction,
	source: ControlSource,
	reason: Option<subnodes::EndReason>,
) -> Result<()> {
	if reason.is_some() != (action == RunControlAction::Cancel) {
		return Err(Error::Invalid(
			"Subnode cancellation requires an end reason".into(),
		));
	}
	let descendants = lock_descendants(scope, creator, action == RunControlAction::Resume).await?;
	if let Some(reason) = reason {
		for (id, thread) in descendants.subnodes {
			scope.mark_ended(id, thread, reason).await?;
		}
	}
	for run in descendants.runs {
		scope.control_run(run, action, source).await?;
	}
	Ok(())
}

/// Deletes a creating Run's active Subnodes when its Agent version is
/// definitely unusable on their Node (#1012: disabling or removing the
/// creating Agent deletes its Subnodes). Called after a Catalog Approval is
/// withdrawn or disabled, after a policy save, or after a generated Agent
/// retires. The scope re-reads both under lock so it preserves the Subnodes of
/// a creator whose approval and subject were re-enabled meanwhile.
///
/// Every Subnode of one creating Run has that Run's Agent and Node, so the
/// whole set ends together, through the same cascade a creator's own end
/// takes ([`control_for_creator`]): marked `creator_agent_removed`, member Runs
/// cancelled, records kept. Returns whether anything ended.
pub async fn end_if_creator_removed(
	scope: &mut dyn crate::ports::subnodes::CreatorRemovalScope,
	creator: uuid::Uuid,
) -> Result<bool> {
	let Some((agent, node)) = scope.lock_creator(creator).await? else {
		return Ok(false);
	};
	let catalog = scope.catalog_enabled(&node, &agent).await?;
	let subject = aidash_domain::agent_subject(&node, &agent.id, &agent.version);
	let subject = scope.subject_enabled(&subject).await?;
	if !subnodes::creator_removed(catalog, subject) {
		return Ok(false);
	}
	control_for_creator(
		scope,
		creator,
		aidash_domain::RunControlAction::Cancel,
		aidash_domain::ControlSource::Tenant,
		Some(subnodes::EndReason::CreatorAgentRemoved),
	)
	.await?;
	Ok(true)
}

/// Arguments after the built-in tool's schema and agent references are parsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Command {
	Task {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		task_model: Option<aidash_domain::TaskModel>,
		id: subnodes::SubnodeId,
		title: String,
		request: String,
		#[serde(default)]
		journal_range: Option<subnodes::JournalRange>,
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

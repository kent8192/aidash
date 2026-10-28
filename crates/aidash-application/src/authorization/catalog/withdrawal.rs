//! The impact of withdrawing one Catalog Approval, shown before the decision.
//! Withdrawal itself is never blocked (#540): this only tells the approver
//! which Agents can no longer start and how many Runs are still in flight.
//! Those Runs pause at their next use of the withdrawn version (a Decider at
//! its next compaction); they are not stopped here.
use super::approvals::dependencies;
use crate::{Error, Result, ports::catalog_withdrawal::WithdrawalImpactScope};
use aidash_domain::registry::EntityRef;
use serde::{Deserialize, Serialize};

/// Who loses what when `entry` is withdrawn on the Node.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WithdrawalImpact {
	/// Approved Agent versions that depend on the withdrawn version.
	pub agents: Vec<EntityRef>,
	/// Non-terminal Runs of those Agent versions on the Node.
	pub running_runs: u64,
}

/// Compute the impact. Only a subject who may withdraw the version may read it.
pub async fn impact(
	scope: &mut dyn WithdrawalImpactScope,
	reference: &EntityRef,
) -> Result<WithdrawalImpact> {
	let entry = scope
		.definition(reference)
		.await?
		.ok_or_else(|| Error::NotFound("registry entry".into()))?;
	if !scope.can_approve(&entry).await? {
		return Err(Error::Forbidden);
	}
	let node = scope.node().deployment.as_str().to_owned();
	let mut agents = Vec::new();
	for agent in scope.approved_agents().await? {
		let own = EntityRef {
			id: agent.id.clone(),
			version: agent.version.clone(),
		};
		// An agent that no longer resolves its dependencies cannot start
		// anyway, so it is not an impact of this withdrawal.
		let Ok(needs) = dependencies(&agent, &node) else {
			continue;
		};
		if needs.contains(reference) && own != *reference {
			agents.push(own);
		}
	}
	agents.sort_by(|a, b| (&a.id, &a.version).cmp(&(&b.id, &b.version)));
	let running_runs = if agents.is_empty() {
		0
	} else {
		scope.non_terminal_runs(&agents).await?
	};
	Ok(WithdrawalImpact {
		agents,
		running_runs,
	})
}

#[cfg(test)]
mod tests;

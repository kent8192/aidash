//! Safety Stop use case (ADR 0101): a Deployment Administrator pauses or
//! cancels one Run this Deployment executes, without reading its content.
//!
//! The caller opens one transaction in the target Tenant's scope and hands this
//! module a [`Ledger`] over it. Everything below happens in that transaction,
//! so the control change, the audit record and the replay receipt commit
//! together or not at all. The caller notifies workers only after the commit.
use crate::{Error, Result};
use aidash_domain::{
	entities::{RunControl, RunPhase},
	safety_stop::{Action, Actor, Command, Decision, Outcome, decide},
};
use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

/// What the command acknowledges. It describes the operation, not the Run's
/// state after later commands, and does not claim that execution has stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct Receipt {
	pub operation_id: Uuid,
	pub run_id: Uuid,
	pub tenant: String,
	pub action: Action,
	pub outcome: Outcome,
}

/// The Run's control facts, and nothing else of the Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
	pub phase: RunPhase,
	pub control: RunControl,
	pub control_revision: i64,
}

/// A receipt as stored, with the digest of the command that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
	pub fingerprint: String,
	pub receipt: Receipt,
}

/// The transaction a Safety Stop runs in.
#[async_trait]
pub trait Ledger: Send {
	/// Takes the Tenant's deletion lock so a deletion and a control change
	/// serialize, and refuses a deleted Tenant.
	async fn tenant_available(&mut self) -> Result<()>;
	/// Locks the Run row. `None` when the Tenant has no such Run.
	async fn lock_run(&mut self, run: Uuid) -> Result<Option<Control>>;
	/// The receipt of an earlier command with this operation ID from this caller.
	async fn receipt(&mut self, caller_key: &str, operation: Uuid) -> Result<Option<Stored>>;
	/// Applies the action as a Deployment Administrator's control decision:
	/// control, control revision and source, and the `run.control` event.
	async fn apply(&mut self, run: Uuid, action: Action, actor: Actor) -> Result<()>;
	/// Records the audit entry and receipt. A failure aborts the whole command.
	async fn record(&mut self, actor: Actor, command: &Command, outcome: Outcome) -> Result<()>;
}

/// What happened, for the caller's post-commit work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
	pub receipt: Receipt,
	/// Control changed in this call, so workers must be told after the commit.
	pub changed: Option<Action>,
}

pub async fn execute(ledger: &mut dyn Ledger, actor: Actor, command: &Command) -> Result<Done> {
	ledger.tenant_available().await?;
	// Lock before the replay check: concurrent commands for one Run serialize,
	// and a retry cannot slip between another command's check and its write.
	let state = ledger
		.lock_run(command.run_id)
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))?;
	if let Some(stored) = ledger
		.receipt(&actor.caller_key(), command.operation_id)
		.await?
	{
		if stored.fingerprint != command.fingerprint() {
			return Err(Error::Conflict(
				"the operation id was used for a different command".into(),
			));
		}
		return Ok(Done {
			receipt: stored.receipt,
			changed: None,
		});
	}
	let outcome = match decide(state.phase, state.control, state.control_revision, command) {
		Decision::Stale => {
			return Err(Error::Conflict(
				"the Run's control changed; read it again before stopping it".into(),
			));
		}
		Decision::NoOp => Outcome::NoOp,
		Decision::Accepted => {
			ledger.apply(command.run_id, command.action, actor).await?;
			Outcome::Accepted
		}
	};
	ledger.record(actor, command, outcome).await?;
	Ok(Done {
		receipt: Receipt {
			operation_id: command.operation_id,
			run_id: command.run_id,
			tenant: command.tenant.clone(),
			action: command.action,
			outcome,
		},
		changed: (outcome == Outcome::Accepted).then_some(command.action),
	})
}

#[cfg(test)]
mod tests;

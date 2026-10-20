//! Auto: the Run's approval Decider judges a pending approval before the Run
//! waits for a person (ADR 0077). A Decider that cannot be asked blocks the call
//! and tells the model; one whose call allowance is spent leaves the call to a
//! person. Neither fails the Run or pauses it.
use super::*;
use aidash_application::{
	capabilities::approvals::Settlement,
	decision::{
		ApprovalTool, DecisionGate, Judgement, Ruling, approval_input_digest, settlement_of,
	},
};
use aidash_domain::{
	decision::{BlockReason, Boundary},
	provider::ToolCall,
	tool::ToolContract,
};

/// The call a pending approval protects, with the tool it invokes.
pub(super) struct PendingCall<'a> {
	pub call: &'a ToolCall,
	pub tool: &'a dyn ExecutionTool,
	pub contract: &'a ToolContract,
	pub instructions: &'a str,
}

/// What the approval Decider made of a pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Judged {
	/// The approval is given; the call may run.
	Allowed,
	/// The approval is refused for this reason, which the model receives.
	Blocked(BlockReason),
	/// Nobody decided, so a person is asked as in Manual: Auto is not available
	/// to the Run, the Decider ran in Shadow mode, the Run has spent the
	/// Decider's call allowance, or the approval is not the Decider's to judge.
	Undecided,
}

impl Judged {
	/// Whether the approval record now says how the call ends.
	pub(super) fn settled(self) -> bool {
		self != Self::Undecided
	}
}

/// The tool as the Decider reads it: the identity the approval names and the
/// specification the model was given.
fn approval_tool(pending: &PendingCall<'_>, target: &str) -> ApprovalTool {
	let (id, version) = match &pending.contract.identity {
		ToolIdentity::Descriptor(reference) => (reference.id.clone(), reference.version.clone()),
		ToolIdentity::Registry(reference) => (reference.id.clone(), reference.version.clone()),
		ToolIdentity::AgentPlugin(tool) => plugin_approval_reference(tool),
		ToolIdentity::Builtin(name) => (name.clone(), "builtin".into()),
	};
	let specification = pending.tool.specification();
	ApprovalTool {
		id,
		version,
		description: specification.description,
		input_schema: specification.parameters,
		target: Some(target.to_owned()),
	}
}

impl Executor<'_> {
	/// Judge `approval` if the Run's approval Decider is to, and settle it.
	pub(super) async fn judge_approval(
		&self,
		run: &Run,
		token: Uuid,
		guard: &dyn ExecutionAuthority,
		pending: &PendingCall<'_>,
		approval: Uuid,
	) -> Result<Judged> {
		let Some(request) = guard.approval_judgement(run, approval).await? else {
			return Ok(Judged::Undecided);
		};
		let Some(ports) = self.environment.approval_decider(run, token).await? else {
			return Ok(Judged::Undecided);
		};
		let tool = approval_tool(pending, &request.target);
		let boundary = Boundary {
			node: self.environment.node_id().to_owned(),
			run: run.id,
			step: run.step,
			run_revision: run.revision,
			input_revision: run.observed_input_seq,
			worker: token,
			input_digest: approval_input_digest(approval, pending.call, &tool),
			cycle: run.recovery.decision_cycle,
			retry: run.recovery.decider_retry,
		};
		let gate = DecisionGate {
			authority: ports.authority.as_ref(),
			journal: ports.journal.as_ref(),
			provider: ports.provider.as_ref(),
		};
		let ruling = gate
			.judge_approval(&Judgement {
				boundary: &boundary,
				decider: &ports.decider,
				definition: &ports.definition,
				now: ports.journal.now(),
				approval,
				context: &run.context,
				people: &request.people,
				instructions: pending.instructions,
				pending: pending.call,
				tool: &tool,
			})
			.await?;
		let Ruling::Decided(evidence) = ruling else {
			return Ok(Judged::Undecided);
		};
		let settlement = settlement_of(&evidence);
		guard.settle_judgement(run, approval, settlement).await?;
		Ok(match settlement {
			Settlement::Allowed { .. } => Judged::Allowed,
			Settlement::Blocked { reason, .. } => Judged::Blocked(reason),
			Settlement::Shadow { .. } => Judged::Undecided,
		})
	}
}

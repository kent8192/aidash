//! The built-in Approval judge's authority and allowance.
use super::CurrentAllowance;
use crate::{Result, database::native};
use aidash_application::ports::decision::{Approval, DecisionAuthority};
use aidash_domain::{
	Run,
	decision::{
		Boundary, DeciderConfig, DeciderPin, Restrictions, SourcePin, policy::ExecutionAllowance,
	},
	registry::rules::digest,
};

/// Auto uses the exact built-in approval judge.
pub(crate) struct ApprovalAuthority {
	pub config: DeciderConfig,
}
#[async_trait::async_trait]
impl DecisionAuthority for ApprovalAuthority {
	async fn check(
		&self,
		_: &Boundary,
		decider: &DeciderPin,
		sources: &[SourcePin],
	) -> aidash_application::Result<Approval> {
		// No context source is declared for a Decider yet (#110).
		if !sources.is_empty() {
			return Err(aidash_application::Error::Forbidden);
		}
		let builtin = aidash_application::registry::system::approval_decider()?;
		if decider.identity.id != builtin.id
			|| decider.identity.version != builtin.version
			|| digest(&serde_json::to_value(&builtin)?) != decider.definition_digest
		{
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(Approval {
			restrictions: Restrictions::default(),
			state_retention: self.config.state_retention.clone(),
		})
	}
}

/// The policy of a generated or RequiredHome Run: the Decider its chain agreed
/// on when the step began. Every charge rechecks that agreement through its
/// owners, so a withdrawn approval stops further attempts.
pub(crate) struct ChainAllowance(pub ExecutionAllowance);
#[async_trait::async_trait]
impl CurrentAllowance for ChainAllowance {
	async fn current(
		&self,
		_: &mut native::Transaction,
		_: &Run,
	) -> Result<Option<ExecutionAllowance>> {
		Ok(Some(self.0.clone()))
	}
}

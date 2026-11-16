//! The native Decision Authority and current policy of a Run's Decider.
use super::CurrentAllowance;
use crate::{Result, authorization::execution::Guard, database::native};
use aidash_application::ports::decision::{Approval, DecisionAuthority};
use aidash_domain::{
	Run,
	context::recovery::Failure,
	decision::{
		Boundary, DeciderConfig, DeciderPin, Restrictions, SourcePin, policy::ExecutionAllowance,
	},
	registry::rules::digest,
};

/// A Decider may run on this Node only under its Catalog Approval for the
/// exact pinned version, which also allows the state retention that version
/// declares (ADR 0042).
pub(crate) struct CatalogAuthority<'a> {
	pub guard: &'a Guard,
	pub config: DeciderConfig,
}
#[async_trait::async_trait]
impl DecisionAuthority for CatalogAuthority<'_> {
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
		require_approved(self.guard, decider).await?;
		Ok(Approval {
			restrictions: Restrictions::default(),
			state_retention: self.config.state_retention.clone(),
		})
	}
}

/// The exact pinned Decider version holds its Catalog Approval on this Node.
pub(crate) async fn require_approved(
	guard: &Guard,
	decider: &DeciderPin,
) -> aidash_application::Result<()> {
	let unapproved = || aidash_application::Error::Context(Failure::DeciderUnapproved);
	let entry = match guard.decider_entry(&decider.identity.local()).await {
		Ok(entry) => entry,
		Err(crate::Error::Forbidden | crate::Error::NotFound(_)) => return Err(unapproved()),
		Err(error) => return Err(error.into()),
	};
	if digest(&serde_json::to_value(&entry)?) != decider.definition_digest {
		return Err(unapproved());
	}
	Ok(())
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

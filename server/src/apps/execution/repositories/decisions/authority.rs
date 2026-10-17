//! The native Decision Authority and current policy of a Run's compaction
//! Decider (ADR 0042).
use super::CurrentAllowance;
use crate::{Result, authorization::execution::Guard, database::native};
use aidash_application::ports::decision::{Approval, DecisionAuthority};
use aidash_domain::{
	Run,
	context::recovery::Failure,
	decision::{
		Boundary, DeciderConfig, DeciderPin, Evidence, Hook, Restrictions, SourcePin,
		policy::ExecutionAllowance,
	},
	registry::rules::digest,
};

/// The policy of a Run whose Agent version is immutable: the Decider its
/// Binding snapshot admitted stays the one it may use. A revoked Catalog
/// Approval is caught by the Decision Authority before every dispatch and
/// before a decision is applied.
pub(crate) struct AdmittedAllowance;
#[async_trait::async_trait]
impl CurrentAllowance for AdmittedAllowance {
	async fn current(
		&self,
		_: &mut native::Transaction,
		run: &Run,
	) -> Result<Option<ExecutionAllowance>> {
		let Some(snapshot) = run.context.binding_snapshot.as_deref() else {
			return Ok(None);
		};
		Ok(snapshot
			.bound_decider(Hook::Compaction)?
			.map(|(bound, _)| bound.allowance()))
	}
}

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
		let unapproved = || aidash_application::Error::Context(Failure::DeciderUnapproved);
		let entry = match self.guard.decider_entry(&decider.identity.local()).await {
			Ok(entry) => entry,
			Err(crate::Error::Forbidden | crate::Error::NotFound(_)) => return Err(unapproved()),
			Err(error) => return Err(error.into()),
		};
		if digest(&serde_json::to_value(&entry)?) != decider.definition_digest {
			return Err(unapproved());
		}
		Ok(Approval {
			restrictions: Restrictions::default(),
			state_retention: self.config.state_retention.clone(),
		})
	}
	/// Decision Evidence is not exposed through the public API (#108).
	async fn read(&self, _: &Evidence, _: bool) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
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

/// Every generated ancestor, or the Home of a RequiredHome execution, is
/// charged for each Decision Attempt before it is dispatched.
pub(crate) struct ChainOwners<'a> {
	pub guard: &'a Guard,
	pub federation: &'a crate::federation::Federation,
	pub decider: aidash_domain::registry::EntityRef,
	pub config: DeciderConfig,
	/// Home reservations kept open while their attempts are dispatched.
	pub open: std::sync::Mutex<
		std::collections::BTreeMap<uuid::Uuid, aidash_domain::generation::dispatch::Input>,
	>,
}
#[async_trait::async_trait]
impl super::DecisionOwners for ChainOwners<'_> {
	async fn charge(
		&self,
		record: &aidash_application::ports::decision::DispatchRecord,
	) -> Result<std::collections::BTreeMap<String, uuid::Uuid>> {
		let (receipts, reservation) = self
			.guard
			.charge_decision(
				self.federation,
				&self.decider,
				&self.config,
				record.attempt,
				&record.request_digest,
				record.questions.len(),
			)
			.await?;
		if let Some(reservation) = reservation {
			self.open
				.lock()
				.expect("open reservations are never poisoned")
				.insert(record.attempt, reservation);
		}
		Ok(receipts)
	}
	async fn settle(
		&self,
		record: &aidash_application::ports::decision::DispatchRecord,
	) -> Result<()> {
		let reservation = self
			.open
			.lock()
			.expect("open reservations are never poisoned")
			.remove(&record.attempt);
		if let Some(reservation) = reservation {
			self.guard.settle_decision(&reservation).await?;
		}
		Ok(())
	}
}

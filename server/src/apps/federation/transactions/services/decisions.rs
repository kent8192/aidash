//! Persistence maps its ORM state to the shared monotonic domain transition.
use crate::Result;
use crate::apps::federation::transactions::models::{
	AtomicCoordinator, states::AtomicCoordinatorDecision,
};
use aidash_domain::transactions::{
	CoordinatorDecision, CoordinatorState, CoordinatorTransition as DomainTransition,
};
pub enum CoordinatorTransition {
	Decide(AtomicCoordinatorDecision),
	Publish,
	Complete,
}
impl CoordinatorTransition {
	fn domain(&self) -> DomainTransition {
		match self {
			Self::Decide(AtomicCoordinatorDecision::Commit) => {
				DomainTransition::Decide(CoordinatorDecision::Commit)
			}
			Self::Decide(AtomicCoordinatorDecision::Abort) => {
				DomainTransition::Decide(CoordinatorDecision::Abort)
			}
			Self::Publish => DomainTransition::Publish,
			Self::Complete => DomainTransition::Complete,
		}
	}
	pub(crate) fn phase(&self) -> &'static str {
		self.domain().phase()
	}
	pub(crate) fn apply(&self, state: &mut AtomicCoordinator) -> Result<bool> {
		let mut domain = CoordinatorState {
			decision: state.decision.as_ref().map(|decision| match decision {
				AtomicCoordinatorDecision::Commit => CoordinatorDecision::Commit,
				AtomicCoordinatorDecision::Abort => CoordinatorDecision::Abort,
			}),
			visible: state.visible,
			complete: state.complete,
		};
		let changed = self.domain().apply(&mut domain)?;
		state.decision = domain.decision.map(|decision| match decision {
			CoordinatorDecision::Commit => AtomicCoordinatorDecision::Commit,
			CoordinatorDecision::Abort => AtomicCoordinatorDecision::Abort,
		});
		state.visible = domain.visible;
		state.complete = domain.complete;
		Ok(changed)
	}
}

//! Provider-neutral decision orchestration. Ports own live authority and fenced persistence.
use crate::ports::decision::*;
use crate::{Error, Result};
use aidash_domain::{
	context::{Context, ContextEvent},
	decision::*,
	registry::rules::digest,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
mod approval_state;
mod dispatch;
mod judge;
pub mod observation;
mod source_selection;
mod state;
pub use approval_state::{ApprovalState, ApprovalTool, build_approval_state};
pub use dispatch::{attempt_identity, decision_identity};
pub use judge::{
	Judgement, Ruling, approval_evidence_identity, approval_input_digest, settlement_of,
};
pub use source_selection::{source_selection_input_digest, source_selection_pins};
#[derive(Debug, Clone, Copy)]
enum Reason {
	Forbidden,
	ProviderFailure,
	AuthorityFailure,
	JournalFailure,
	InvalidAnswers,
}

pub struct DecisionGate<'a> {
	pub authority: &'a dyn DecisionAuthority,
	pub journal: &'a dyn DecisionJournal,
	pub provider: &'a dyn DecisionProvider,
}
pub struct Evaluation<'a> {
	pub boundary: &'a Boundary,
	pub decider: &'a BoundDecider,
	pub definition: &'a aidash_domain::registry::Entry,
	pub disclosure: &'a Disclosure,
	pub now: DateTime<Utc>,
}
fn validate_plan(requests: &[PreparedRequest], questions: &Questions) -> Result<()> {
	let mut ids = BTreeSet::new();
	if requests.is_empty() {
		return Err(Error::Invalid("empty decision request plan".into()));
	}
	for request in requests {
		if request.body.is_empty() || request.questions.is_empty() {
			return Err(Error::Invalid("empty decision request batch".into()));
		}
		for (id, question) in &request.questions {
			if questions.get(id) != Some(question) || !ids.insert(id) {
				return Err(Error::Invalid(
					"decision request plan changes or duplicates a question".into(),
				));
			}
		}
	}
	if ids.len() != questions.len() {
		return Err(Error::Invalid(
			"decision request plan omits a candidate".into(),
		));
	}
	Ok(())
}

#[cfg(test)]
mod tests;

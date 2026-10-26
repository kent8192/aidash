//! Provider-neutral decision orchestration. Ports own live authority and fenced persistence.
use crate::ports::decision::*;
use crate::{Error, Result};
use aidash_domain::{
	context::{Context, ContextEvent, RequestBudget},
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
pub use state::{BuiltState, build_compaction_state, compaction_disclosure};

/// Digest the complete inference input before redacting its classification view.
pub fn compaction_input_digest(
	context: &Context,
	budget: &RequestBudget<'_>,
	pinned: &Value,
) -> Result<String> {
	Ok(digest(&serde_json::to_value(
		budget.request(context, pinned),
	)?))
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
#[derive(Debug, Clone)]
pub enum CompactionResult {
	AlreadyFits,
	/// The pruned context fits; it is saved and the step ends.
	Applied(Summary),
	/// The proposal is recorded but never applied.
	Shadow(Summary),
	/// Nothing could be classified, so pruning changes nothing.
	Unpruned(Summary),
	/// Pruning did not make the request fit. Nothing is recorded yet: the
	/// Summary Stage either completes it or the decision is recorded as
	/// rejected (ADR 0056).
	Insufficient(Box<StagedDecision>),
}

/// An Enforce decision whose pruning was not enough on its own. Its evidence
/// is recorded with the adopted summary, or rejected when none is adopted.
#[derive(Debug, Clone)]
pub struct StagedDecision {
	/// Evidence with the `staged` outcome and the pruned projection's digest.
	pub evidence: Evidence,
	/// The pruned projection the Summary Stage continues from.
	pub pruned: Context,
	pub state: Option<RetainedState>,
}
impl StagedDecision {
	/// The same decision when no summary is adopted: it changes nothing.
	pub fn rejected(&self) -> Evidence {
		let mut evidence = self.evidence.clone();
		evidence.outcome = Outcome::Rejected;
		evidence.projection_digest = None;
		evidence
	}
}

impl DecisionGate<'_> {
	pub async fn compact(
		&self,
		context: &mut Context,
		budget: &RequestBudget<'_>,
		pinned: &Value,
		input: &Evaluation<'_>,
	) -> Result<CompactionResult> {
		let size = |context: &Context| budget.request(context, pinned).estimated_total_tokens();
		let before = size(context);
		if before <= budget.window {
			return Ok(CompactionResult::AlreadyFits);
		}
		input.boundary.validate()?;
		if input.boundary.input_digest != compaction_input_digest(context, budget, pinned)? {
			return Err(Error::Invalid(
				"decision input differs from its admitted revision".into(),
			));
		}
		let native =
			budget.projection.version() == aidash_domain::projection::ProjectionVersion::Native;
		let config = input.decider.pin.check(input.definition)?;
		if config != input.decider.config {
			return Err(Error::Invalid(
				"bound Decider configuration differs from its admitted pin".into(),
			));
		}
		if input.decider.pin.identity.registry_deployment != input.boundary.node
			|| self.provider.implementation_id() != input.decider.pin.provider_implementation
			|| self.provider.configuration_digest()? != input.decider.pin.configuration_digest
		{
			return Err(Error::Invalid(
				"decision provider differs from the executing Run pin".into(),
			));
		}
		input.decider.restrictions.validate(&config)?;
		// Each Route is asked only when every earlier one is unavailable, never
		// for a better answer (ADR 0066).
		let mut outage = false;
		for (index, route) in config.routes.iter().enumerate() {
			let attempt = RouteAttempt {
				index: u32::try_from(index)
					.map_err(|_| Error::Invalid("too many Decider Routes".into()))?,
				route,
				config: &config,
				before,
				native,
			};
			match self
				.compact_route(context, budget, pinned, input, &attempt)
				.await
			{
				Ok(result) => return Ok(result),
				Err(RouteFailure::Unavailable { transient, error }) => {
					tracing::warn!(
						route = index,
						model = route.model.as_str(),
						%error,
						"Decider Route unavailable"
					);
					outage |= transient;
				}
				Err(RouteFailure::Final(error)) => return Err(error),
			}
		}
		// A provider outage is retried by the step; a state no Route accepts is not.
		Err(if outage {
			Error::DeciderOutage
		} else {
			Error::Context(aidash_domain::context::recovery::Failure::DeciderUnavailable)
		})
	}

	/// One decision on one Route. A failure that only makes this Route
	/// unavailable lets the caller try the next one; every other failure is final.
	async fn compact_route(
		&self,
		context: &mut Context,
		budget: &RequestBudget<'_>,
		pinned: &Value,
		input: &Evaluation<'_>,
		attempt: &RouteAttempt<'_>,
	) -> std::result::Result<CompactionResult, RouteFailure> {
		let RouteAttempt {
			index,
			route,
			config,
			before,
			native,
		} = *attempt;
		let size = |context: &Context| budget.request(context, pinned).estimated_total_tokens();
		// The Route's own threshold, under whatever the Binding already requires.
		let route_restrictions = input.decider.restrictions.with_route(route);
		let mut restrictions = route_restrictions.clone();
		let mut state_expiry = None;
		let built = build_compaction_state(
			context,
			input.boundary,
			input.disclosure,
			&input.decider.restrictions,
			&route.state_budget,
			native,
		)?;
		let state_digest = digest(&built.state);
		let id = dispatch::decision_id(input, &state_digest, budget.window, index);
		let recovered = self.journal.recover(id, input.boundary).await?;
		let mut evidence = Evidence {
			version: EVIDENCE_VERSION,
			id,
			boundary: input.boundary.clone(),
			hook: config.hook,
			decider: input.decider.pin.clone(),
			route: index,
			route_models: config.models(),
			route_thresholds: config.thresholds(),
			provider_contract: route.provider_contract.clone(),
			model: route.model.clone(),
			configuration_parameters_digest: config.parameters_digest()?,
			builder: config.builder.clone(),
			option_source: config.option_source.clone(),
			rule: config.rule.clone(),
			state_digest: state_digest.clone(),
			history_length: context.history.len(),
			turns: built.units.turns,
			sources: input.disclosure.sources.clone(),
			candidates: built.candidates,
			questions: built.questions,
			answers: BTreeMap::new(),
			restrictions: restrictions.clone(),
			binding_restrictions: input.decider.restrictions.clone(),
			mode: config.mode,
			branches: BTreeMap::new(),
			attempts: vec![],
			outcome: Outcome::Rejected,
			reason: Reason::NoCandidates,
			fit: FitMetrics {
				before,
				proposed: before,
				window: budget.window,
				retained: context.history.len(),
				removed: 0,
				dropped: 0,
				truncated: 0,
			},
			projection_digest: None,
			state: StateReference::Disabled,
			state_retention: StateRetentionWitness::Disabled,
		};
		dispatch::validate_recovered_identity(
			input,
			id,
			&state_digest,
			&evidence.questions,
			&recovered,
			index,
		)?;
		for prior in &recovered {
			evidence
				.attempts
				.push(dispatch::recovered_attempt_evidence(prior));
			if let Some(answers) = &prior.answers {
				evidence.answers.extend(answers.clone());
			}
		}
		let initial_approval = self
			.authority
			.check(
				input.boundary,
				&input.decider.pin,
				&input.disclosure.sources,
			)
			.await
			.and_then(|approval| {
				restrictions = restrictions.intersect(&approval.restrictions, config)?;
				evidence.restrictions = restrictions.clone();
				state_expiry = approval
					.state_retention
					.expires_at(input.now, input.disclosure.source_expiry)?;
				Ok(())
			});
		if let Err(error) = initial_approval {
			evidence.reason = dispatch::error_reason(&error, Reason::AuthorityFailure);
			self.journal.commit(&evidence, None, None).await?;
			return Err(RouteFailure::Final(error));
		}

		if evidence.questions.is_empty() {
			// Every event is protected or undisclosed: pruning changes nothing,
			// and the Summary Stage may still reduce the context.
			evidence.candidates.clear();
			self.journal.commit(&evidence, None, None).await?;
			return Ok(CompactionResult::Unpruned(evidence.summary()));
		}
		if restrictions.forbid_apply && config.mode == Mode::Enforce {
			evidence.reason = Reason::Forbidden;
			self.journal.commit(&evidence, None, None).await?;
			return Err(RouteFailure::Final(Error::Forbidden));
		}
		let requests = match built.fits.then(|| {
			self.provider
				.plan(index as usize, &built.state, &evidence.questions)
		}) {
			Some(Ok(requests)) if validate_plan(&requests, &evidence.questions).is_ok() => requests,
			unplanned => {
				// Nothing was sent: the state is larger than this Route accepts, or
				// the questions do not fit its provider. A later Route may take it.
				evidence.reason = Reason::ProviderFailure;
				self.journal.commit(&evidence, None, None).await?;
				return Err(RouteFailure::Unavailable {
					transient: false,
					error: match unplanned {
						None => Error::Context(
							aidash_domain::context::recovery::Failure::DeciderUnavailable,
						),
						Some(_) => {
							Error::Invalid("decision cannot plan complete provider requests".into())
						}
					},
				});
			}
		};
		let records = requests
			.iter()
			.map(|request| dispatch::record(input, id, &state_digest, request, index))
			.collect::<Result<Vec<_>>>()?;
		if let Err(error) = dispatch::validate_recovered_plan(&records, &requests, &recovered) {
			evidence.reason = Reason::JournalFailure;
			self.journal.commit(&evidence, None, None).await?;
			return Err(RouteFailure::Final(error));
		}
		// Every fresh batch rechecks authority after reservation. Recovered attempts
		// reuse only durable answers; uncertain charges never authorize another I/O.
		let attempts = futures_util::future::join_all(requests.iter().zip(&records).map(
			|(request, record)| {
				self.dispatch_batch(
					request,
					record,
					&recovered,
					input,
					config,
					&route_restrictions,
				)
			},
		))
		.await;
		let mut failure = None;
		// Only when every failed batch was a fresh dispatch this Route could
		// not answer may the next Route be asked.
		let mut unavailable = true;
		for attempt in attempts {
			if let Some(record) = attempt.evidence {
				if let Some(saved) = evidence.attempts.iter_mut().find(|a| a.id == record.id) {
					*saved = record;
				} else {
					evidence.attempts.push(record);
				}
			}
			evidence.answers.extend(attempt.answers);
			if let Some(current) = attempt.restrictions {
				restrictions = restrictions.intersect(&current, config)?;
			}
			if let Some(expiry) = attempt.state_expiry {
				state_expiry = dispatch::intersect_expiry(state_expiry, expiry);
			}
			if let Some((reason, error)) = attempt.failure {
				unavailable &= attempt.next_route;
				if failure.is_none() || reason == Reason::Forbidden {
					failure = Some((reason, error));
				}
			}
		}
		evidence.restrictions = restrictions.clone();
		if failure.is_none() && validate_answers(&evidence.questions, &evidence.answers).is_err() {
			// The journal lost answers it charged: final, never another Route.
			unavailable = false;
			failure = Some((
				Reason::IncompleteAnswers,
				Error::Invalid("incomplete durable decision answers".into()),
			));
		}
		if let Some((reason, error)) = failure {
			evidence.reason = reason;
			self.journal.commit(&evidence, None, None).await?;
			return Err(if unavailable {
				RouteFailure::Unavailable {
					transient: true,
					error,
				}
			} else {
				RouteFailure::Final(error)
			});
		}
		// Current stricter rules may retain more events, but never change the pin or mode.
		// Invalid final approvals must retain the same fenced evidence as revocation.
		let final_approval = self
			.authority
			.check(
				input.boundary,
				&input.decider.pin,
				&input.disclosure.sources,
			)
			.await
			.and_then(|current| {
				let restrictions = restrictions.intersect(&current.restrictions, config)?;
				evidence.restrictions = restrictions.clone();
				let expiry = current
					.state_retention
					.expires_at(input.now, input.disclosure.source_expiry)?;
				Ok((restrictions, expiry))
			});
		let (current_restrictions, current_expiry) = match final_approval {
			Ok(approval) => approval,
			Err(error) => {
				evidence.reason = dispatch::error_reason(&error, Reason::AuthorityFailure);
				self.journal.commit(&evidence, None, None).await?;
				return Err(RouteFailure::Final(error));
			}
		};
		restrictions = current_restrictions;
		state_expiry = match (state_expiry, current_expiry) {
			(Some(old), Some(new)) => Some(old.min(new)),
			_ => None,
		};
		evidence.restrictions = restrictions.clone();
		let mut candidate = context.clone();
		let mut selected = BTreeMap::new();
		for option in &evidence.candidates {
			let protected = aidash_domain::decision::pruning::protected(
				option.history_index,
				option.turn,
				context.history.len(),
				built.units.turns,
				restrictions.preserve_recent,
			);
			let choice = if protected {
				Branch::Keep
			} else {
				branch(
					evidence.answers[&option.keep_call],
					evidence.answers[&option.keep_result],
					restrictions.threshold,
				)
			};
			selected.insert(option.history_index, choice);
			evidence.branches.insert(option.id.clone(), choice);
		}
		let pruned =
			aidash_domain::decision::pruning::apply(&context.history, &selected, &built.units);
		candidate.history = pruned.history;
		evidence.fit.removed = pruned.removed;
		evidence.fit.truncated = pruned.truncated;
		evidence.fit.dropped = selected
			.values()
			.filter(|choice| **choice == Branch::Drop)
			.count();
		evidence.fit.retained = context.history.len() - pruned.removed;
		evidence.fit.proposed = size(&candidate);
		evidence.reason = if restrictions.forbid_apply {
			Reason::Forbidden
		} else if evidence.fit.proposed > budget.window {
			Reason::Insufficient
		} else {
			Reason::Fits
		};
		evidence.outcome = if config.mode == Mode::Shadow {
			Outcome::Shadow
		} else if evidence.reason == Reason::Fits {
			Outcome::Applied
		} else if evidence.reason == Reason::Insufficient {
			Outcome::Staged
		} else {
			Outcome::Rejected
		};
		if matches!(evidence.outcome, Outcome::Applied | Outcome::Staged) {
			evidence.projection_digest = Some(aidash_domain::decision::pruning::projection_digest(
				&candidate.history,
			)?);
		}
		let retained = state_expiry.and_then(|expires_at| {
			let state_id = Uuid::new_v4();
			evidence.state_retention = StateRetentionWitness::Enabled {
				id: state_id,
				expires_at,
			};
			if expires_at <= self.journal.now() {
				evidence.state = StateReference::Expired {
					id: state_id,
					digest: state_digest.clone(),
					expired_at: expires_at,
				};
				None
			} else {
				Some(RetainedState {
					id: state_id,
					digest: state_digest,
					expires_at,
					state: built.state,
				})
			}
		});
		if let Some(state) = &retained {
			evidence.state = StateReference::Retained {
				id: state.id,
				digest: state.digest.clone(),
				expires_at: state.expires_at,
			};
		}
		evidence.replay()?;
		if evidence.outcome == Outcome::Staged {
			let staged = StagedDecision {
				evidence,
				pruned: candidate,
				state: retained,
			};
			staged.rejected().replay()?;
			return Ok(CompactionResult::Insufficient(Box::new(staged)));
		}
		if evidence.outcome == Outcome::Applied {
			candidate.compactions = candidate
				.compactions
				.checked_add(1)
				.ok_or_else(|| Error::Invalid("compaction counter overflow".into()))?;
			self.journal
				.commit(&evidence, Some(&candidate), retained.as_ref())
				.await?;
			*context = candidate;
			return Ok(CompactionResult::Applied(evidence.summary()));
		}
		self.journal
			.commit(&evidence, None, retained.as_ref())
			.await?;
		if evidence.outcome == Outcome::Shadow {
			return Ok(CompactionResult::Shadow(evidence.summary()));
		}
		Err(RouteFailure::Final(Error::Forbidden))
	}
	/// Record a staged decision as rejected because no summary was adopted.
	pub async fn reject_staged(&self, staged: &StagedDecision) -> Result<()> {
		self.journal
			.commit(&staged.rejected(), None, staged.state.as_ref())
			.await
	}
	pub async fn replay(&self, evidence: &Evidence) -> Result<BTreeMap<String, Branch>> {
		self.authority.read(evidence, false).await?;
		Ok(evidence.replay()?)
	}
}
/// One Route of a decision and what every Route of it shares.
#[derive(Clone, Copy)]
struct RouteAttempt<'a> {
	index: u32,
	route: &'a DeciderRoute,
	config: &'a DeciderConfig,
	before: usize,
	native: bool,
}

enum RouteFailure {
	/// The Route could not answer: a provider failure or invalid answers
	/// (`transient`), or a state or plan it does not accept. Its evidence is
	/// already recorded.
	Unavailable {
		transient: bool,
		error: Error,
	},
	Final(Error),
}
impl From<Error> for RouteFailure {
	fn from(error: Error) -> Self {
		Self::Final(error)
	}
}
impl From<aidash_domain::Error> for RouteFailure {
	fn from(error: aidash_domain::Error) -> Self {
		Self::Final(error.into())
	}
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

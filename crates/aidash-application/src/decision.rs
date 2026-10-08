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
mod state;
pub use state::{BuiltState, build_compaction_state};

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
	pub decider: &'a DeciderPin,
	pub definition: &'a aidash_domain::registry::Entry,
	pub restrictions: &'a Restrictions,
	pub disclosure: &'a Disclosure,
	pub now: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionResult {
	AlreadyFits,
	Applied(Summary),
	Shadow(Summary),
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
		let config = input.decider.check(input.definition)?;
		if input.decider.identity.registry_node != input.boundary.node
			|| self.provider.implementation_id() != input.decider.provider_implementation
			|| self.provider.configuration_digest()? != input.decider.configuration_digest
		{
			return Err(Error::Invalid(
				"decision provider differs from the executing Run pin".into(),
			));
		}
		let approval = self
			.authority
			.check(input.boundary, input.decider, &input.disclosure.sources)
			.await?;
		let mut restrictions = input
			.restrictions
			.intersect(&approval.restrictions, &config)?;
		let mut state_expiry = approval
			.state_retention
			.expires_at(input.now, input.disclosure.source_expiry)?;
		let built =
			build_compaction_state(context, input.boundary, input.disclosure, &restrictions)?;
		let state_digest = digest(&built.state);
		let id = Uuid::new_v4();
		let mut evidence = Evidence {
			version: EVIDENCE_VERSION,
			id,
			boundary: input.boundary.clone(),
			hook: config.hook,
			decider: input.decider.clone(),
			provider_contract: config.provider_contract.clone(),
			model: config.model.clone(),
			builder: config.builder.clone(),
			option_source: config.option_source.clone(),
			rule: config.rule.clone(),
			state_digest: state_digest.clone(),
			history_length: context.history.len(),
			sources: input.disclosure.sources.clone(),
			candidates: built.candidates,
			questions: built.questions,
			answers: BTreeMap::new(),
			restrictions: restrictions.clone(),
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
				dropped: 0,
				truncated: 0,
			},
			state: StateReference::Disabled,
		};
		if evidence.questions.is_empty()
			|| restrictions.forbid_apply && config.mode == Mode::Enforce
		{
			evidence.reason = if evidence.questions.is_empty() {
				Reason::NoCandidates
			} else {
				Reason::Forbidden
			};
			self.journal.commit(&evidence, None, None).await?;
			if evidence.reason == Reason::Forbidden {
				return Err(Error::Forbidden);
			}
			return Err(Error::Invalid(
				"decision cannot compact the protected context".into(),
			));
		}
		let requests = match self.provider.plan(&built.state, &evidence.questions) {
			Ok(requests) if validate_plan(&requests, &evidence.questions).is_ok() => requests,
			_ => {
				evidence.reason = Reason::ProviderFailure;
				self.journal.commit(&evidence, None, None).await?;
				return Err(Error::Invalid(
					"decision cannot plan complete provider requests".into(),
				));
			}
		};
		// No arbitrary batch/concurrency ceiling. Every batch receives its own live
		// authority check and durable reservation. No successful subset is applied.
		let attempts = futures_util::future::join_all(requests.iter().map(|request| async {
			let transport = self.provider.prepare(request)?;
			let current = self
				.authority
				.check(input.boundary, input.decider, &input.disclosure.sources)
				.await?;
			input
				.restrictions
				.intersect(&current.restrictions, &config)?;
			current
				.state_retention
				.expires_at(input.now, input.disclosure.source_expiry)?;
			if current.restrictions.forbid_apply && config.mode == Mode::Enforce {
				return Err(Error::Forbidden);
			}
			let record = DispatchRecord {
				attempt: Uuid::new_v4(),
				decision: id,
				boundary: input.boundary.clone(),
				decider: input.decider.clone(),
				request_digest: request.digest(),
				state_digest: state_digest.clone(),
				questions: request.questions.keys().cloned().collect(),
				mode: config.mode,
			};
			let permit = self.journal.reserve(&record).await?;
			if permit.record != record
				|| permit.owner_receipts.is_empty()
				|| permit.owner_receipts.values().any(Uuid::is_nil)
			{
				return Err(Error::Invalid(
					"decision dispatch lacks exact durable owner receipts".into(),
				));
			}
			let result = transport.dispatch().await;
			let (mut status, mut answers, mut failure) = match result {
				Ok(answers) if validate_answers(&request.questions, &answers).is_ok() => {
					(AttemptStatus::Answered, Some(answers), None)
				}
				Ok(_) | Err(DispatchError::InvalidAnswers) => {
					(AttemptStatus::Failed, None, Some(Reason::InvalidAnswers))
				}
				Err(DispatchError::ProviderFailure(_)) => (
					AttemptStatus::Uncertain,
					None,
					Some(Reason::ProviderFailure),
				),
			};
			if self
				.journal
				.finish_attempt(&permit, answers.as_ref(), status)
				.await
				.is_err()
			{
				// The call remains charged, but its answer is not a durable recovery source.
				status = AttemptStatus::Uncertain;
				answers = None;
				failure = Some(Reason::ProviderFailure);
			}
			Ok::<_, Error>((
				AttemptEvidence {
					id: record.attempt,
					request_digest: record.request_digest,
					questions: record.questions,
					status,
					owner_receipts: permit.owner_receipts,
				},
				answers,
				current,
				failure,
			))
		}))
		.await;
		let mut failure = None;
		for attempt in attempts {
			match attempt {
				Ok((record, answers, current, reason)) => {
					evidence.attempts.push(record);
					restrictions = restrictions.intersect(&current.restrictions, &config)?;
					let expiry = current
						.state_retention
						.expires_at(input.now, input.disclosure.source_expiry)?;
					state_expiry = match (state_expiry, expiry) {
						(Some(old), Some(new)) => Some(old.min(new)),
						_ => None,
					};
					if let Some(answers) = answers {
						evidence.answers.extend(answers);
					}
					failure = failure.or(reason);
				}
				Err(Error::Forbidden) => failure = Some(Reason::Forbidden),
				Err(_) => failure = failure.or(Some(Reason::ProviderFailure)),
			}
		}
		if failure.is_none() && validate_answers(&evidence.questions, &evidence.answers).is_err() {
			failure = Some(Reason::InvalidAnswers);
		}
		if let Some(reason) = failure {
			evidence.reason = reason;
			self.journal.commit(&evidence, None, None).await?;
			if reason == Reason::Forbidden {
				return Err(Error::Forbidden);
			}
			return Err(Error::Invalid(
				"decision requests did not produce complete durable answers".into(),
			));
		}
		// Current stricter rules may retain more events, but never change the pin or mode.
		// Invalid final approvals must retain the same fenced evidence as revocation.
		let final_approval = self
			.authority
			.check(input.boundary, input.decider, &input.disclosure.sources)
			.await
			.and_then(|current| {
				let restrictions = restrictions.intersect(&current.restrictions, &config)?;
				let expiry = current
					.state_retention
					.expires_at(input.now, input.disclosure.source_expiry)?;
				Ok((restrictions, expiry))
			});
		let (current_restrictions, current_expiry) = match final_approval {
			Ok(approval) => approval,
			Err(error) => {
				evidence.reason = Reason::Forbidden;
				self.journal.commit(&evidence, None, None).await?;
				return Err(error);
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
			let protected = option.history_index == 0
				|| option.history_index
					>= context
						.history
						.len()
						.saturating_sub(restrictions.preserve_recent);
			let choice = if protected {
				Branch::Keep
			} else {
				branch(
					evidence.answers[&option.keep_call],
					evidence.answers[&option.keep_result],
					restrictions.keep_threshold,
				)
			};
			selected.insert(option.history_index, choice);
			evidence.branches.insert(option.id.clone(), choice);
		}
		candidate.history = context.history.iter().enumerate().filter_map(|(index, event)| {
            match selected.get(&index).copied().unwrap_or(Branch::Keep) {
                Branch::Keep => Some(event.clone()),
                Branch::Drop => { evidence.fit.dropped += 1; None },
                Branch::TruncateResult => {
                    let ContextEvent::Tool { call, result } = event else { return Some(event.clone()); };
                    evidence.fit.truncated += 1;
                    let text = result.as_str().map(str::to_owned).unwrap_or_else(||result.to_string());
                    let length = text.chars().count();
                    if length <= 420 { evidence.fit.truncated -= 1; return Some(event.clone()); }
                    // The 300-character prefix is the historical action rule, not a
                    // classification-state/evidence limit. It stays in inference only.
                    let head: String = text.chars().take(300).collect();
                    Some(ContextEvent::Tool { call: call.clone(), result: json!(format!("{head}\n[decision compaction truncated {} chars of this tool result; full result remains in the execution journal]",length-300)) })
                }
            }
        }).collect();
		evidence.fit.retained = candidate.history.len();
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
		} else {
			Outcome::Rejected
		};
		let retained = state_expiry.and_then(|expires_at| {
			let state_id = Uuid::new_v4();
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
		Err(Error::Invalid(
			"decision could not fit the complete inference context".into(),
		))
	}
	pub async fn replay(&self, evidence: &Evidence) -> Result<BTreeMap<String, Branch>> {
		self.authority.read(evidence, false).await?;
		Ok(evidence.replay()?)
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

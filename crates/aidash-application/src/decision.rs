//! Provider-neutral decision orchestration. Ports own live authority and fenced persistence.
use crate::ports::decision::*;
use crate::{Error, Result};
use aidash_domain::{
	context::{Context, ContextEvent, HistoryEntry, RequestBudget},
	decision::*,
	registry::rules::digest,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
mod dispatch;
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
	pub decider: &'a BoundDecider,
	pub definition: &'a aidash_domain::registry::Entry,
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
		let config = input.decider.pin.check(input.definition)?;
		if config != input.decider.config {
			return Err(Error::Invalid(
				"bound Decider configuration differs from its admitted pin".into(),
			));
		}
		if input.decider.pin.identity.registry_node != input.boundary.node
			|| self.provider.implementation_id() != input.decider.pin.provider_implementation
			|| self.provider.configuration_digest()? != input.decider.pin.configuration_digest
		{
			return Err(Error::Invalid(
				"decision provider differs from the executing Run pin".into(),
			));
		}
		input.decider.restrictions.validate(&config)?;
		let mut restrictions = input.decider.restrictions.clone();
		let mut state_expiry = None;
		let built = build_compaction_state(
			context,
			input.boundary,
			input.disclosure,
			&input.decider.restrictions,
		)?;
		let state_digest = digest(&built.state);
		let id = dispatch::decision_id(input, &state_digest, budget.window);
		let recovered = self.journal.recover(id, input.boundary).await?;
		let mut evidence = Evidence {
			version: EVIDENCE_VERSION,
			id,
			boundary: input.boundary.clone(),
			hook: config.hook,
			decider: input.decider.pin.clone(),
			provider_contract: config.provider_contract.clone(),
			model: config.model.clone(),
			configuration_parameters_digest: config.parameters_digest()?,
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
			state_retention: StateRetentionWitness::Disabled,
		};
		dispatch::validate_recovered_identity(
			input,
			id,
			&state_digest,
			&evidence.questions,
			&recovered,
		)?;
		for prior in &recovered {
			evidence
				.attempts
				.push(dispatch::attempt_evidence(&prior.permit, prior.status));
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
				restrictions = restrictions.intersect(&approval.restrictions, &config)?;
				evidence.restrictions = restrictions.clone();
				state_expiry = approval
					.state_retention
					.expires_at(input.now, input.disclosure.source_expiry)?;
				Ok(())
			});
		if let Err(error) = initial_approval {
			evidence.reason = dispatch::error_reason(&error, Reason::AuthorityFailure);
			self.journal.commit(&evidence, None, None).await?;
			return Err(error);
		}

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
		let records: Vec<_> = requests
			.iter()
			.map(|request| dispatch::record(input, id, &state_digest, request))
			.collect();
		if let Err(error) = dispatch::validate_recovered_plan(&records, &requests, &recovered) {
			evidence.reason = Reason::JournalFailure;
			self.journal.commit(&evidence, None, None).await?;
			return Err(error);
		}
		// Every fresh batch rechecks authority after reservation. Recovered attempts
		// reuse only durable answers; uncertain charges never authorize another I/O.
		let attempts = futures_util::future::join_all(requests.iter().zip(&records).map(
			|(request, record)| self.dispatch_batch(request, record, &recovered, input, &config),
		))
		.await;
		let mut failure = None;
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
				restrictions = restrictions.intersect(&current, &config)?;
			}
			if let Some(expiry) = attempt.state_expiry {
				state_expiry = dispatch::intersect_expiry(state_expiry, expiry);
			}
			if let Some((reason, error)) = attempt.failure
				&& (failure.is_none() || reason == Reason::Forbidden)
			{
				failure = Some((reason, error));
			}
		}
		evidence.restrictions = restrictions.clone();
		if failure.is_none() && validate_answers(&evidence.questions, &evidence.answers).is_err() {
			failure = Some((
				Reason::InvalidAnswers,
				Error::Invalid("incomplete durable decision answers".into()),
			));
		}
		if let Some((reason, error)) = failure {
			evidence.reason = reason;
			self.journal.commit(&evidence, None, None).await?;
			return Err(error);
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
				let restrictions = restrictions.intersect(&current.restrictions, &config)?;
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
		candidate.history = context.history.iter().enumerate().filter_map(|(index, entry)| {
            match selected.get(&index).copied().unwrap_or(Branch::Keep) {
                Branch::Keep => Some(entry.clone()),
                Branch::Drop => { evidence.fit.dropped += 1; None },
                Branch::TruncateResult => {
                    let ContextEvent::Tool { call, result } = &entry.event else { return Some(entry.clone()); };
                    evidence.fit.truncated += 1;
                    let text = result.as_str().map(str::to_owned).unwrap_or_else(||result.to_string());
                    let length = text.chars().count();
                    if length <= 420 { evidence.fit.truncated -= 1; return Some(entry.clone()); }
                    // The 300-character prefix is the historical action rule, not a
                    // classification-state/evidence limit. It stays in inference only.
                    let head: String = text.chars().take(300).collect();
                    Some(HistoryEntry { seq: entry.seq, event: ContextEvent::Tool { call: call.clone(), result: json!(format!("{head}\n[decision compaction truncated {} chars of this tool result; full result remains in the execution journal]",length-300)) } })
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

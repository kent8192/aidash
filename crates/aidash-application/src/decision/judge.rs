//! The approval Decider's decision (ADR 0077). It judges one pending call of a
//! Run, asking each Decider Route in order as compaction does. It ends in
//! recorded evidence: a Route's verdict, or `Blocked(Unavailable)` when the
//! Routes could not be asked. The one exception is a spent call allowance, which
//! leaves the call to a person. It never fails the Run or pauses it for the
//! Decider's sake.
use super::*;
use aidash_domain::{context::recovery::Failure, provider::ToolCall, run_input::RunInput};

/// One pending call and everything the approval Decider is asked about it.
pub struct Judgement<'a> {
	pub boundary: &'a Boundary,
	pub decider: &'a BoundDecider,
	pub definition: &'a aidash_domain::registry::Entry,
	pub now: DateTime<Utc>,
	/// The approval record the decision settles.
	pub approval: Uuid,
	pub context: &'a Context,
	/// The inputs people sent to the Run's Thread.
	pub people: &'a [RunInput],
	pub instructions: &'a str,
	pub pending: &'a ToolCall,
	pub tool: &'a ApprovalTool,
}

/// What a Decision Boundary commits to for an approval: the record judged and
/// the call it protects, so two approvals in one step never share a decision.
pub fn approval_input_digest(approval: Uuid, pending: &ToolCall, tool: &ApprovalTool) -> String {
	digest(&json!({
		"approval": approval,
		"call": {"name": pending.name, "arguments": pending.arguments},
		"tool": {"id": tool.id, "version": tool.version},
	}))
}

/// The identity of an approval decision's evidence: the approval it settles,
/// the Decider and the Route that answered, and the state that Route saw. It
/// leaves out the worker and the Run's revision, so judging again after a crash
/// reproduces the same evidence, and a journal recomputes it at commit.
pub fn approval_evidence_identity(
	node: &str,
	run: Uuid,
	approval: Uuid,
	decider: &DeciderPin,
	route: Option<u32>,
	state_digest: &str,
) -> Uuid {
	dispatch::stable_id(&json!({
		"contract": "aidash.approval-decision/1", "node": node, "run": run,
		"approval": approval, "decider": decider, "route": route, "state": state_digest,
	}))
}

/// How the evidence settles its approval: Shadow evidence settles nothing.
pub fn settlement_of(
	evidence: &ApprovalEvidence,
) -> crate::ports::capabilities::approvals::Settlement {
	use crate::ports::capabilities::approvals::Settlement;
	let reason = evidence.verdict.reason();
	match (evidence.outcome(), reason) {
		(ApprovalOutcome::Shadow, _) => Settlement::Shadow {
			evidence: evidence.id,
		},
		(ApprovalOutcome::Blocked, Some(reason)) => Settlement::Blocked {
			reason,
			evidence: evidence.id,
		},
		(ApprovalOutcome::Blocked | ApprovalOutcome::Allowed, _) => Settlement::Allowed {
			evidence: evidence.id,
		},
	}
}

/// How judging a pending call ended.
#[derive(Debug)]
pub enum Ruling {
	/// The Decider decided, or could not be asked; the evidence is recorded.
	Decided(Box<ApprovalEvidence>),
	/// The Run has spent the Decider's call allowance. That is no outage and no
	/// verdict: nothing is recorded, and the call is left to a person.
	AllowanceSpent,
}

/// A Route's answers to every approval question.
struct Answered {
	index: u32,
	state_digest: String,
	answers: BTreeMap<String, Probability>,
	attempts: Vec<AttemptEvidence>,
	restrictions: Restrictions,
}

/// A Route that gave no verdict. `stop` says no other Route is asked: its
/// cause lies with the Decider as a whole, such as a lost lease or a
/// withdrawn approval.
struct Unanswered {
	state_digest: Option<String>,
	attempts: Vec<AttemptEvidence>,
	/// Answers any batch recorded before the Route became unavailable.
	answers: BTreeMap<String, Probability>,
	stop: bool,
	error: Error,
}
impl Unanswered {
	fn route(
		state_digest: &str,
		attempts: Vec<AttemptEvidence>,
		answers: BTreeMap<String, Probability>,
		stop: bool,
		error: Error,
	) -> Self {
		Self {
			state_digest: Some(state_digest.to_owned()),
			attempts,
			answers,
			stop,
			error,
		}
	}
}
impl<E: Into<Error>> From<E> for Unanswered {
	fn from(error: E) -> Self {
		Self {
			state_digest: None,
			attempts: vec![],
			answers: BTreeMap::new(),
			stop: true,
			error: error.into(),
		}
	}
}

impl DecisionGate<'_> {
	/// Judge a pending call and record the decision. An error means the
	/// decision could not be recorded or the Decider was misconfigured; a
	/// Decider that is merely unavailable yields evidence that blocks the call
	/// as `Unavailable`, and a spent call allowance yields no decision at all.
	pub async fn judge_approval(&self, judged: &Judgement<'_>) -> Result<Ruling> {
		judged.boundary.validate()?;
		let config = judged.decider.pin.check(judged.definition)?;
		if config != judged.decider.config || config.hook != Hook::Approval {
			return Err(Error::Invalid(
				"bound Decider is not the admitted approval Decider".into(),
			));
		}
		if judged.decider.pin.identity.registry_node != judged.boundary.node
			|| self.provider.implementation_id() != judged.decider.pin.provider_implementation
			|| self.provider.configuration_digest()? != judged.decider.pin.configuration_digest
		{
			return Err(Error::Invalid(
				"decision provider differs from the executing Run pin".into(),
			));
		}
		judged.decider.restrictions.validate(&config)?;
		let disclosure = Disclosure::default();
		let input = Evaluation {
			boundary: judged.boundary,
			decider: judged.decider,
			definition: judged.definition,
			disclosure: &disclosure,
			now: judged.now,
		};
		let mut failed_routes = vec![];
		let mut last_unanswered = None;
		for (index, route) in config.routes.iter().enumerate() {
			let index = u32::try_from(index)
				.map_err(|_| Error::Invalid("too many Decider Routes".into()))?;
			match self
				.judge_route(&input, judged, &config, index, route)
				.await
			{
				Ok(answered) => {
					return self
						.conclude(judged, &config, failed_routes, Ok(answered))
						.await
						.map(|evidence| Ruling::Decided(Box::new(evidence)));
				}
				Err(unanswered) => {
					tracing::warn!(
						route = index,
						model = route.model.as_str(),
						error = %unanswered.error,
						"approval Decider Route unavailable"
					);
					failed_routes.push(index);
					let stop = unanswered.stop;
					last_unanswered = Some(unanswered);
					if stop {
						break;
					}
				}
			}
		}
		let unanswered = last_unanswered.unwrap_or(Unanswered {
			state_digest: None,
			attempts: vec![],
			answers: BTreeMap::new(),
			stop: true,
			error: Error::Invalid("approval Decider lists no Route".into()),
		});
		if matches!(
			unanswered.error,
			Error::Context(Failure::DeciderCallsExhausted)
		) {
			return Ok(Ruling::AllowanceSpent);
		}
		self.conclude(judged, &config, failed_routes, Err(unanswered))
			.await
			.map(|evidence| Ruling::Decided(Box::new(evidence)))
	}

	async fn conclude(
		&self,
		judged: &Judgement<'_>,
		config: &DeciderConfig,
		failed_routes: Vec<u32>,
		result: std::result::Result<Answered, Unanswered>,
	) -> Result<ApprovalEvidence> {
		let binding = judged.decider.restrictions.clone();
		let (route, state_digest, answers, attempts, restrictions, verdict) = match result {
			Ok(answered) => {
				let verdict = verdict(&answered.answers, answered.restrictions.threshold)?;
				(
					Some(answered.index),
					answered.state_digest,
					answered.answers,
					answered.attempts,
					answered.restrictions,
					verdict,
				)
			}
			Err(unanswered) => (
				None,
				unanswered
					.state_digest
					.unwrap_or_else(|| digest(&Value::Null)),
				unanswered.answers,
				unanswered.attempts,
				binding.clone(),
				Verdict::Blocked(BlockReason::Unavailable),
			),
		};
		let evidence = ApprovalEvidence {
			version: APPROVAL_EVIDENCE_VERSION,
			id: approval_evidence_identity(
				&judged.boundary.node,
				judged.boundary.run,
				judged.approval,
				&judged.decider.pin,
				route,
				&state_digest,
			),
			boundary: judged.boundary.clone(),
			approval: judged.approval,
			decider: judged.decider.pin.clone(),
			route,
			failed_routes,
			route_models: config.models(),
			route_thresholds: config.thresholds(),
			configuration_parameters_digest: config.parameters_digest()?,
			state_digest,
			answers,
			restrictions,
			binding_restrictions: binding,
			mode: config.mode,
			verdict,
			attempts,
		};
		evidence.validate()?;
		self.journal.commit_approval(&evidence).await?;
		Ok(evidence)
	}

	/// One Route's answers, or why it gave none.
	async fn judge_route(
		&self,
		input: &Evaluation<'_>,
		judged: &Judgement<'_>,
		config: &DeciderConfig,
		index: u32,
		route: &DeciderRoute,
	) -> std::result::Result<Answered, Unanswered> {
		let questions = approval_questions();
		let route_restrictions = input.decider.restrictions.with_route(route);
		let built = build_approval_state(
			judged.context,
			judged.people,
			judged.instructions,
			judged.pending,
			judged.tool,
			&route.state_budget,
		)?;
		let state_digest = digest(&built.state);
		let id = dispatch::decision_id(input, &state_digest, 0, index);
		let recovered = self.journal.recover(id, input.boundary).await?;
		dispatch::validate_recovered_identity(
			input,
			id,
			&state_digest,
			&questions,
			&recovered,
			index,
		)?;
		let mut attempts: Vec<AttemptEvidence> = recovered
			.iter()
			.map(|prior| dispatch::attempt_evidence(&prior.permit, prior.status))
			.collect();
		let mut answers: BTreeMap<String, Probability> = recovered
			.iter()
			.filter_map(|prior| prior.answers.clone())
			.flatten()
			.collect();
		let unanswered =
			|attempts: &[AttemptEvidence], answers: &BTreeMap<String, Probability>, stop, error| {
				Unanswered::route(
					&state_digest,
					attempts.to_vec(),
					answers.clone(),
					stop,
					error,
				)
			};
		let initial = self
			.authority
			.check(input.boundary, &input.decider.pin, &[])
			.await
			.and_then(|approval| Ok(route_restrictions.intersect(&approval.restrictions, config)?));
		let mut restrictions = match initial {
			Ok(restrictions) => restrictions,
			Err(error) => return Err(unanswered(&attempts, &answers, true, error)),
		};
		let requests = match built
			.fits
			.then(|| self.provider.plan(index as usize, &built.state, &questions))
		{
			Some(Ok(requests)) if validate_plan(&requests, &questions).is_ok() => requests,
			unplanned => {
				// Nothing was sent: the state is larger than this Route accepts,
				// or the questions do not fit its provider.
				let error = match unplanned {
					None => Error::Context(Failure::DeciderUnavailable),
					Some(_) => {
						Error::Invalid("decision cannot plan complete provider requests".into())
					}
				};
				return Err(unanswered(&attempts, &answers, false, error));
			}
		};
		let records = requests
			.iter()
			.map(|request| dispatch::record(input, id, &state_digest, request, index))
			.collect::<Result<Vec<_>>>()?;
		dispatch::validate_recovered_plan(&records, &requests, &recovered)?;
		let batches = futures_util::future::join_all(requests.iter().zip(&records).map(
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
		for batch in batches {
			if let Some(record) = batch.evidence {
				if let Some(saved) = attempts.iter_mut().find(|a| a.id == record.id) {
					*saved = record;
				} else {
					attempts.push(record);
				}
			}
			answers.extend(batch.answers);
			if let Some(current) = batch.restrictions {
				restrictions = restrictions
					.intersect(&current, config)
					.map_err(|error| unanswered(&attempts, &answers, true, error.into()))?;
			}
			if let Some((_, error)) = batch.failure {
				unavailable &= batch.next_route;
				failure.get_or_insert(error);
			}
		}
		if let Some(error) = failure {
			return Err(unanswered(&attempts, &answers, !unavailable, error));
		}
		if validate_answers(&questions, &answers).is_err() {
			let error = Error::Invalid("incomplete durable decision answers".into());
			return Err(unanswered(&attempts, &answers, true, error));
		}
		let current = self
			.authority
			.check(input.boundary, &input.decider.pin, &[])
			.await
			.and_then(|current| Ok(restrictions.intersect(&current.restrictions, config)?));
		match current {
			Ok(current) => restrictions = current,
			Err(error) => return Err(unanswered(&attempts, &answers, true, error)),
		}
		Ok(Answered {
			index,
			state_digest,
			answers,
			attempts,
			restrictions,
		})
	}
}

#[cfg(test)]
mod tests;

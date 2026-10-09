//! Single-use dispatch and recovery preserve charges, authority and original port errors.
use super::*;

#[derive(Default)]
pub(super) struct BatchResult {
	pub evidence: Option<AttemptEvidence>,
	pub answers: BTreeMap<String, Probability>,
	pub restrictions: Option<Restrictions>,
	pub state_expiry: Option<Option<DateTime<Utc>>>,
	pub failure: Option<(Reason, Error)>,
}

fn stable_id(value: &Value) -> Uuid {
	// A deterministic 128-bit identity from the canonical SHA-256 commitment.
	Uuid::parse_str(&digest(value)[7..39]).expect("canonical digest contains 32 hex digits")
}

pub(super) fn decision_id(input: &Evaluation<'_>, state: &str, window: usize) -> Uuid {
	let boundary = input.boundary;
	stable_id(&json!({
		"contract":"aidash.decision/1", "node":boundary.node, "run":boundary.run,
		"step":boundary.step, "input_revision":boundary.input_revision,
		"input_digest":boundary.input_digest, "decider":input.decider.pin,
		"binding_restrictions":input.decider.restrictions, "state":state,
		"sources":input.disclosure.sources, "window":window,
	}))
}

pub(super) fn record(
	input: &Evaluation<'_>,
	decision: Uuid,
	state: &str,
	request: &PreparedRequest,
) -> DispatchRecord {
	let questions: Vec<_> = request.questions.keys().cloned().collect();
	let request_digest = request.digest();
	DispatchRecord {
		attempt: stable_id(
			&json!({"decision":decision,"request":request_digest,"questions":questions}),
		),
		decision,
		boundary: input.boundary.clone(),
		decider: input.decider.pin.clone(),
		binding_restrictions: input.decider.restrictions.clone(),
		request_digest,
		state_digest: state.into(),
		questions,
		mode: input.decider.config.mode,
	}
}

fn validate_permit(
	record: &DispatchRecord,
	permit: &DispatchPermit,
	recovered: bool,
) -> Result<()> {
	let mut saved = permit.record.clone();
	if recovered {
		// The journal fences recovery using the new lease; immutable inputs must match.
		saved.boundary.worker = record.boundary.worker;
		saved.boundary.run_revision = record.boundary.run_revision;
	}
	if &saved != record
		|| permit.owner_receipts.is_empty()
		|| permit.owner_receipts.values().any(Uuid::is_nil)
	{
		return Err(Error::Invalid(
			"decision dispatch lacks exact durable owner receipts".into(),
		));
	}
	Ok(())
}

pub(super) fn validate_recovered_identity(
	input: &Evaluation<'_>,
	decision: Uuid,
	state: &str,
	questions: &Questions,
	recovered: &[RecoveredAttempt],
) -> Result<()> {
	let mut coverage = BTreeSet::new();
	for prior in recovered {
		let saved = &prior.permit.record;
		validate_digest(&saved.request_digest)?;
		if saved.questions.is_empty()
			|| saved
				.questions
				.iter()
				.any(|q| !questions.contains_key(q) || !coverage.insert(q))
		{
			return Err(Error::Invalid(
				"corrupt recovered decision question coverage".into(),
			));
		}
		let expected = DispatchRecord {
			attempt: stable_id(
				&json!({"decision":decision,"request":saved.request_digest,"questions":saved.questions}),
			),
			decision,
			boundary: input.boundary.clone(),
			decider: input.decider.pin.clone(),
			binding_restrictions: input.decider.restrictions.clone(),
			request_digest: saved.request_digest.clone(),
			state_digest: state.into(),
			questions: saved.questions.clone(),
			mode: input.decider.config.mode,
		};
		validate_permit(&expected, &prior.permit, true)?;
		let request = PreparedRequest {
			body: vec![],
			questions: saved
				.questions
				.iter()
				.map(|q| (q.clone(), questions[q].clone()))
				.collect(),
		};
		validate_recovered_answers(prior, &request)?;
	}
	Ok(())
}

pub(super) fn validate_recovered_plan(
	records: &[DispatchRecord],
	requests: &[PreparedRequest],
	recovered: &[RecoveredAttempt],
) -> Result<()> {
	let mut ids = BTreeSet::new();
	for prior in recovered {
		let Some(index) = records
			.iter()
			.position(|record| record.attempt == prior.permit.record.attempt)
		else {
			return Err(Error::Conflict(
				"decision recovery differs from its charged request plan".into(),
			));
		};
		if !ids.insert(prior.permit.record.attempt) {
			return Err(Error::Invalid(
				"duplicate recovered decision attempt".into(),
			));
		}
		validate_permit(&records[index], &prior.permit, true)?;
		validate_recovered_answers(prior, &requests[index])?;
	}
	Ok(())
}

fn validate_recovered_answers(prior: &RecoveredAttempt, request: &PreparedRequest) -> Result<()> {
	match (&prior.answers, prior.status) {
		(Some(answers), AttemptStatus::Answered) => {
			Ok(validate_answers(&request.questions, answers)?)
		}
		(None, AttemptStatus::Failed | AttemptStatus::Uncertain | AttemptStatus::NotDispatched) => {
			Ok(())
		}
		_ => Err(Error::Invalid("corrupt recovered decision answers".into())),
	}
}

pub(super) fn error_reason(error: &Error, origin: Reason) -> Reason {
	if matches!(error, Error::Forbidden | Error::Unauthorized) {
		Reason::Forbidden
	} else {
		origin
	}
}

pub(super) fn intersect_expiry(
	old: Option<DateTime<Utc>>,
	new: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
	match (old, new) {
		(Some(old), Some(new)) => Some(old.min(new)),
		_ => None,
	}
}

impl BatchResult {
	fn approve(
		&mut self,
		current: Approval,
		input: &Evaluation<'_>,
		config: &DeciderConfig,
	) -> Result<()> {
		let restrictions = self
			.restrictions
			.as_ref()
			.unwrap_or(&input.decider.restrictions)
			.intersect(&current.restrictions, config)?;
		// Retain a valid restrictive approval even when it denies this dispatch.
		let forbidden = restrictions.forbid_apply && config.mode == Mode::Enforce;
		self.restrictions = Some(restrictions);
		if forbidden {
			return Err(Error::Forbidden);
		}
		let expiry = current
			.state_retention
			.expires_at(input.now, input.disclosure.source_expiry)?;
		self.state_expiry = Some(match self.state_expiry {
			Some(old) => intersect_expiry(old, expiry),
			None => expiry,
		});
		Ok(())
	}
	fn recover(&mut self, prior: RecoveredAttempt, request: &PreparedRequest) -> Result<()> {
		validate_recovered_answers(&prior, request)?;
		self.evidence = Some(attempt_evidence(&prior.permit, prior.status));
		match prior.status {
			AttemptStatus::Answered => {
				self.answers = prior.answers.expect("validated answered attempt");
				Ok(())
			}
			AttemptStatus::Failed => Err(Error::Invalid(
				"recovered decision attempt has invalid answers".into(),
			)),
			AttemptStatus::Uncertain | AttemptStatus::NotDispatched => Err(Error::Conflict(
				"charged decision attempt requires journal recovery; redispatch is forbidden"
					.into(),
			)),
		}
	}
}

pub(super) fn attempt_evidence(permit: &DispatchPermit, status: AttemptStatus) -> AttemptEvidence {
	AttemptEvidence {
		id: permit.record.attempt,
		request_digest: permit.record.request_digest.clone(),
		questions: permit.record.questions.clone(),
		status,
		owner_receipts: permit.owner_receipts.clone(),
	}
}

impl DecisionGate<'_> {
	pub(super) async fn dispatch_batch(
		&self,
		request: &PreparedRequest,
		record: &DispatchRecord,
		recovered: &[RecoveredAttempt],
		input: &Evaluation<'_>,
		config: &DeciderConfig,
	) -> BatchResult {
		let mut batch = BatchResult::default();
		let mut origin = Reason::AuthorityFailure;
		let result = async {
			let current = self
				.authority
				.check(
					input.boundary,
					&input.decider.pin,
					&input.disclosure.sources,
				)
				.await?;
			batch.approve(current, input, config)?;
			if input
				.disclosure
				.source_expiry
				.is_some_and(|deadline| deadline <= self.journal.now())
			{
				return Err(Error::Forbidden);
			}
			if let Some(prior) = recovered
				.iter()
				.find(|prior| prior.permit.record.attempt == record.attempt)
			{
				origin = if prior.status == AttemptStatus::Failed {
					Reason::InvalidAnswers
				} else {
					Reason::JournalFailure
				};
				return batch.recover(prior.clone(), request);
			}
			origin = Reason::ProviderFailure;
			let transport = self.provider.prepare(request)?;
			origin = Reason::JournalFailure;
			let permit = match self.journal.reserve(record).await? {
				Reservation::Fresh(permit) => {
					validate_permit(record, &permit, false)?;
					permit
				}
				Reservation::Recovered(prior) => {
					validate_permit(record, &prior.permit, true)?;
					if prior.status == AttemptStatus::Failed {
						origin = Reason::InvalidAnswers;
					}
					return batch.recover(prior, request);
				}
			};
			batch.evidence = Some(attempt_evidence(&permit, AttemptStatus::NotDispatched));
			origin = Reason::AuthorityFailure;
			// Reservation may wait on remote owners. Recheck after that await, and make
			// the live deadline check after the authority await immediately before I/O.
			let approval = self
				.authority
				.check(
					input.boundary,
					&input.decider.pin,
					&input.disclosure.sources,
				)
				.await
				.and_then(|current| batch.approve(current, input, config))
				.and_then(|()| {
					if input
						.disclosure
						.source_expiry
						.is_some_and(|deadline| deadline <= self.journal.now())
					{
						Err(Error::Forbidden)
					} else {
						Ok(())
					}
				});
			if let Err(error) = approval {
				if self
					.journal
					.finish_attempt(&permit, None, AttemptStatus::NotDispatched)
					.await
					.is_err()
				{
					batch.evidence.as_mut().unwrap().status = AttemptStatus::Uncertain;
				}
				return Err(error);
			}
			let (status, answers, failure) = match transport.dispatch().await {
				Ok(answers) if validate_answers(&request.questions, &answers).is_ok() => {
					(AttemptStatus::Answered, Some(answers), None)
				}
				Ok(_) | Err(DispatchError::InvalidAnswers) => (
					AttemptStatus::Failed,
					None,
					Some((
						Reason::InvalidAnswers,
						Error::Invalid("invalid decision answers".into()),
					)),
				),
				Err(DispatchError::ProviderFailure(_)) => (
					AttemptStatus::Uncertain,
					None,
					Some((
						Reason::ProviderFailure,
						Error::Invalid("decision provider request failed".into()),
					)),
				),
			};
			batch.evidence.as_mut().unwrap().status = status;
			origin = Reason::JournalFailure;
			if let Err(error) = self
				.journal
				.finish_attempt(&permit, answers.as_ref(), status)
				.await
			{
				batch.evidence.as_mut().unwrap().status = AttemptStatus::Uncertain;
				return Err(error);
			}
			batch.answers = answers.unwrap_or_default();
			if let Some((reason, error)) = failure {
				origin = reason;
				return Err(error);
			}
			Ok(())
		}
		.await;
		if let Err(error) = result {
			batch.failure = Some((error_reason(&error, origin), error));
		}
		batch
	}
}

//! Lease-fenced persistence of Decision Evidence and Decision Attempts behind the
//! portable `DecisionGate`.
//!
//! One journal serves one leased Run and the exact Decider pin admitted for it.
//! Its boundary maps `run_revision` to `runs.revision` and `input_revision` to
//! `runs.observed_input_seq`; both must still match whenever the journal reserves,
//! recovers or commits. The Run itself is the only owner charged here: each
//! Decision Attempt counts against the Decider's per-Run call limit.
use crate::{Error, Result, database::native, store::Store};
use aidash_application::ports::decision::{
	DecisionJournal, DispatchPermit, DispatchRecord, RecoveredAttempt, Reservation, RetainedState,
};
use aidash_domain::{
	Run,
	context::{Context, recovery::Failure},
	decision::{
		AttemptStatus, Boundary, Evidence, Mode, Outcome, OutcomeLink, Probability, Restrictions,
		StateReference, StateRetentionWitness, policy::ExecutionAllowance, validate_digest,
	},
	registry::rules,
};
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const ATTEMPTS: &str = "decision_attempts";
const EVIDENCE: &str = "decision_evidence";
const STATES: &str = "decision_states";
const OUTCOMES: &str = "decision_outcomes";
const PENDING: &str = "pending";
const ATTEMPT_COLUMNS: [&str; 18] = [
	"id",
	"decision_id",
	"run_id",
	"node",
	"step",
	"input_revision",
	"input_digest",
	"worker",
	"run_revision",
	"decider",
	"binding_restrictions",
	"request_digest",
	"state_digest",
	"questions",
	"mode",
	"owner_receipts",
	"status",
	"answers",
];

/// The snake_case wire label of a serialized enum, as stored in the CHECKed columns.
fn label<T: Serialize>(value: &T) -> Result<String> {
	match serde_json::to_value(value)? {
		Value::String(label) => Ok(label),
		_ => Err(Error::Invalid("decision label is not a string".into())),
	}
}

fn parse<T: DeserializeOwned>(label: String) -> Result<T> {
	Ok(serde_json::from_value(Value::String(label))?)
}

fn status_label(status: AttemptStatus) -> Result<String> {
	label(&status)
}

/// A reservation that never finished may have reached the provider.
fn recovered_status(label: &str) -> Result<AttemptStatus> {
	if label == PENDING {
		return Ok(AttemptStatus::Uncertain);
	}
	parse(label.to_owned())
}

fn stored_record(row: &native::Row) -> Result<(DispatchPermit, String, Option<Value>)> {
	let questions: Vec<String> = serde_json::from_value(row.try_get("questions")?)?;
	let record = DispatchRecord {
		attempt: row.try_get("id")?,
		decision: row.try_get("decision_id")?,
		boundary: Boundary {
			node: row.try_get("node")?,
			run: row.try_get("run_id")?,
			step: row.try_get("step")?,
			run_revision: row.try_get("run_revision")?,
			input_revision: row.try_get("input_revision")?,
			worker: row.try_get("worker")?,
			input_digest: row.try_get("input_digest")?,
		},
		decider: serde_json::from_value(row.try_get("decider")?)?,
		binding_restrictions: serde_json::from_value::<Restrictions>(
			row.try_get("binding_restrictions")?,
		)?,
		request_digest: row.try_get("request_digest")?,
		state_digest: row.try_get("state_digest")?,
		questions,
		mode: parse::<Mode>(row.try_get("mode")?)?,
	};
	let owner_receipts: BTreeMap<String, Uuid> =
		serde_json::from_value(row.try_get("owner_receipts")?)?;
	Ok((
		DispatchPermit {
			record,
			owner_receipts,
		},
		row.try_get("status")?,
		row.try_get("answers")?,
	))
}

fn recovered(row: &native::Row) -> Result<RecoveredAttempt> {
	let (permit, status, answers) = stored_record(row)?;
	let status = recovered_status(&status)?;
	let answers = answers
		.map(serde_json::from_value::<BTreeMap<String, Probability>>)
		.transpose()?;
	if answers.is_some() != (status == AttemptStatus::Answered) {
		return Err(Error::Invalid("corrupt stored decision answers".into()));
	}
	Ok(RecoveredAttempt {
		permit,
		status,
		answers,
	})
}

/// A later worker recovers or continues a decision under its own lease, so
/// only the worker and Run revision of its boundary may differ.
fn same_input(saved: &Boundary, current: &Boundary) -> bool {
	&Boundary {
		worker: current.worker,
		run_revision: current.run_revision,
		..saved.clone()
	} == current
}

/// Fields fixed when an attempt is first charged.
fn same_charge(saved: &DispatchRecord, requested: &DispatchRecord) -> bool {
	same_input(&saved.boundary, &requested.boundary)
		&& &DispatchRecord {
			boundary: requested.boundary.clone(),
			..saved.clone()
		} == requested
}

/// Fields every attempt of one decision shares, so a single evidence snapshot
/// can name them all. Only the attempt, its request and its questions differ.
fn same_decision(saved: &DispatchRecord, requested: &DispatchRecord) -> bool {
	same_charge(
		&DispatchRecord {
			attempt: requested.attempt,
			request_digest: requested.request_digest.clone(),
			questions: requested.questions.clone(),
			..saved.clone()
		},
		requested,
	)
}

/// `committed` preserves at least the history `binding` required; authority
/// may only tighten the restrictions an attempt was charged with.
fn at_least_as_strict(committed: &Restrictions, binding: &Restrictions) -> bool {
	committed.keep_threshold.value() <= binding.keep_threshold.value()
		&& committed.preserve_recent >= binding.preserve_recent
		&& (committed.forbid_apply || !binding.forbid_apply)
}

/// Lock the Run while `boundary.worker` holds its lease and its revisions match.
async fn fence(tx: &mut native::Transaction, boundary: &Boundary) -> Result<()> {
	let fenced = native::query(
		&Query::select()
			.columns(["revision", "observed_input_seq"].map(Alias::new))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(boundary.run)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(boundary.worker)))
			.and_where(Expr::col("lease_until").gt(Expr::current_timestamp()))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
	let revision: i64 = fenced.try_get("revision")?;
	let input: i64 = fenced.try_get("observed_input_seq")?;
	if revision != boundary.run_revision || input != boundary.input_revision {
		return Err(Error::Conflict(
			"decision boundary differs from the current Run revision".into(),
		));
	}
	Ok(())
}

/// An applied or shadow decision is final and is never overwritten.
async fn ensure_open(tx: &mut native::Transaction, run: Uuid, decision: Uuid) -> Result<()> {
	let finished: Option<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(EVIDENCE))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("decision_id").eq(Expr::value(decision)))
			.and_where(Expr::col("outcome").is_in(["applied", "shadow"]))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **tx)
	.await?;
	match finished {
		Some(_) => Err(Error::Conflict(
			"decision already has its final evidence; reload the Run".into(),
		)),
		None => Ok(()),
	}
}

/// The decision's charged attempts, locked until the transaction ends so a
/// concurrent `finish_attempt` is ordered before or after this read.
async fn attempts_of(
	tx: &mut native::Transaction,
	run: Uuid,
	decision: Uuid,
) -> Result<Vec<native::Row>> {
	native::query(
		&Query::select()
			.columns(ATTEMPT_COLUMNS.map(Alias::new))
			.from(Alias::new(ATTEMPTS))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("decision_id").eq(Expr::value(decision)))
			.order_by(Alias::new("created_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **tx)
	.await
}

/// The Run's current decision policy for the hook its journal serves, read again
/// inside every reservation transaction after the Run row is locked.
#[async_trait::async_trait]
pub trait CurrentAllowance: Send + Sync {
	/// `None` when the current policy no longer allows a Decider for the hook.
	async fn current(
		&self,
		tx: &mut native::Transaction,
		run: &Run,
	) -> Result<Option<ExecutionAllowance>>;
}

/// Decision Evidence and Decision Attempts of one leased Run and its admitted Decider.
pub struct RunDecisionJournal<'a> {
	store: &'a Store,
	run: Run,
	allowance: ExecutionAllowance,
	current: &'a dyn CurrentAllowance,
}

impl<'a> RunDecisionJournal<'a> {
	/// `run` is the leased Run whose context an applied decision replaces;
	/// `allowance` names the admitted Decider pin and its per-Run call limit,
	/// and `current` supplies the policy each charge is checked against again.
	pub fn new(
		store: &'a Store,
		run: Run,
		allowance: ExecutionAllowance,
		current: &'a dyn CurrentAllowance,
	) -> Result<Self> {
		allowance.decider.validate()?;
		Ok(Self {
			store,
			run,
			allowance,
			current,
		})
	}

	/// The receipt owner for this Run's own call limit.
	fn owner(&self) -> String {
		format!("{}/runs/{}", self.store.node_id, self.run.id)
	}

	fn check_boundary(&self, boundary: &Boundary) -> Result<()> {
		boundary.validate()?;
		if boundary.run != self.run.id || boundary.node != self.store.node_id {
			return Err(Error::Invalid(
				"decision boundary belongs to another Run or Node".into(),
			));
		}
		// An applied commit saves this snapshot, so it must be the Run the boundary saw.
		if boundary.step != self.run.step
			|| boundary.run_revision != self.run.revision
			|| boundary.input_revision != self.run.observed_input_seq
		{
			return Err(Error::Conflict(
				"decision boundary differs from the journal's Run; reload the Run".into(),
			));
		}
		Ok(())
	}

	fn check_record(&self, record: &DispatchRecord) -> Result<()> {
		self.check_boundary(&record.boundary)?;
		validate_digest(&record.request_digest)?;
		validate_digest(&record.state_digest)?;
		let unique: BTreeSet<_> = record.questions.iter().collect();
		if record.decider != self.allowance.decider {
			return Err(Error::Invalid(
				"Decision Attempt names another Decider than the Run's admitted pin".into(),
			));
		}
		if record.attempt.is_nil()
			|| record.decision.is_nil()
			|| record.questions.is_empty()
			|| unique.len() != record.questions.len()
			|| record.questions.iter().any(String::is_empty)
		{
			return Err(Error::Invalid("malformed Decision Attempt".into()));
		}
		Ok(())
	}

	async fn recover_attempts(
		&self,
		decision: Uuid,
		boundary: &Boundary,
	) -> Result<Vec<RecoveredAttempt>> {
		self.check_boundary(boundary)?;
		let mut tx = native::begin(&self.store.pool).await?;
		fence(&mut tx, boundary).await?;
		ensure_open(&mut tx, self.run.id, decision).await?;
		let attempts = attempts_of(&mut tx, self.run.id, decision)
			.await?
			.iter()
			.map(recovered)
			.collect::<Result<Vec<_>>>()?;
		tx.commit().await?;
		Ok(attempts)
	}

	async fn reserve_attempt(&self, record: &DispatchRecord) -> Result<Reservation> {
		self.check_record(record)?;
		let mut tx = native::begin(&self.store.pool).await?;
		fence(&mut tx, &record.boundary).await?;
		ensure_open(&mut tx, self.run.id, record.decision).await?;
		let decision_attempts = attempts_of(&mut tx, self.run.id, record.decision).await?;
		let requested: BTreeSet<_> = record.questions.iter().collect();
		for row in &decision_attempts {
			let prior = recovered(row)?;
			if prior.permit.record.attempt == record.attempt {
				// A charged attempt is recovered, never charged or dispatched again.
				if !same_charge(&prior.permit.record, record) {
					return Err(Error::Conflict(
						"Decision Attempt changed its charged inputs".into(),
					));
				}
				tx.commit().await?;
				return Ok(Reservation::Recovered(prior));
			}
			// Another batch of the same decision must share its inputs, or no single
			// evidence snapshot could name both charges.
			if !same_decision(&prior.permit.record, record) {
				return Err(Error::Conflict(
					"Decision Attempt differs from its decision's charged inputs".into(),
				));
			}
			if prior
				.permit
				.record
				.questions
				.iter()
				.any(|question| requested.contains(question))
			{
				return Err(Error::Conflict(
					"Decision Attempt overlaps another charged plan".into(),
				));
			}
		}
		let decider = serde_json::to_value(&record.decider)?;
		let consumed: i64 = native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new(ATTEMPTS))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.and_where(SimpleExpr::CustomWithExpr(
					"(decider = CAST(? AS jsonb))".to_owned(),
					vec![Expr::value(decider.to_string()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		let consumed = u64::try_from(consumed)
			.map_err(|_| Error::Invalid("negative decision call count".into()))?;
		// The stricter of the admitted and current limits applies, and a current
		// policy that names another pin is an error rather than a silent switch.
		let remaining = match self.current.current(&mut tx, &self.run).await? {
			Some(current) => self.allowance.remaining(&current, consumed)?,
			None => 0,
		};
		if remaining == 0 {
			// A Decider whose call limit is spent or revoked is unavailable; the Run pauses.
			return Err(Error::Context(Failure::PruneUnavailable));
		}
		let owner_receipts = BTreeMap::from([(self.owner(), Uuid::new_v4())]);
		native::query(
			&Query::insert()
				.into_table(Alias::new(ATTEMPTS))
				.columns(ATTEMPT_COLUMNS[..17].iter().copied().map(Alias::new))
				.values_panic([
					Expr::value(record.attempt),
					Expr::value(record.decision),
					Expr::value(record.boundary.run),
					Expr::value(record.boundary.node.as_str()),
					Expr::value(record.boundary.step),
					Expr::value(record.boundary.input_revision),
					Expr::value(record.boundary.input_digest.as_str()),
					Expr::value(record.boundary.worker),
					Expr::value(record.boundary.run_revision),
					Expr::value(decider),
					Expr::value(serde_json::to_value(&record.binding_restrictions)?),
					Expr::value(record.request_digest.as_str()),
					Expr::value(record.state_digest.as_str()),
					Expr::value(serde_json::to_value(&record.questions)?),
					Expr::value(label(&record.mode)?),
					Expr::value(serde_json::to_value(&owner_receipts)?),
					Expr::value(PENDING),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(Reservation::Fresh(DispatchPermit {
			record: record.clone(),
			owner_receipts,
		}))
	}

	/// Records the physical result of a charged attempt. The lease is not
	/// required: a result that arrives after the lease moved is still a fact.
	async fn finish(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> Result<()> {
		let record = &permit.record;
		if record.boundary.run != self.run.id {
			return Err(Error::Invalid(
				"Decision Attempt belongs to another Run".into(),
			));
		}
		if answers.is_some() != (status == AttemptStatus::Answered)
			|| answers.is_some_and(|answers| {
				answers.len() != record.questions.len()
					|| record.questions.iter().any(|q| !answers.contains_key(q))
			}) {
			return Err(Error::Invalid(
				"Decision Attempt answers must cover exactly its questions".into(),
			));
		}
		let answers = answers.map(serde_json::to_value).transpose()?;
		let status = status_label(status)?;
		let mut tx = native::begin(&self.store.pool).await?;
		let saved = native::query(
			&Query::select()
				.columns(ATTEMPT_COLUMNS.map(Alias::new))
				.from(Alias::new(ATTEMPTS))
				.and_where(Expr::col("id").eq(Expr::value(record.attempt)))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::NotFound("Decision Attempt was never charged".into()))?;
		let (charged, saved_status, saved_answers) = stored_record(&saved)?;
		// Only the exact permit the attempt was charged with may record its result,
		// so the answers above were checked against the stored questions.
		if charged != *permit {
			return Err(Error::Conflict(
				"Decision Attempt permit differs from its charge".into(),
			));
		}
		if saved_status != PENDING {
			if saved_status != status || saved_answers != answers {
				return Err(Error::Conflict(
					"Decision Attempt already finished differently".into(),
				));
			}
			return tx.commit().await;
		}
		native::query(
			&Query::update()
				.table(Alias::new(ATTEMPTS))
				.value(Alias::new("status"), status.as_str())
				.value(Alias::new("answers"), answers)
				.value_expr(Alias::new("finished_at"), Expr::current_timestamp())
				.and_where(Expr::col("id").eq(Expr::value(record.attempt)))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await
	}

	/// The evidence must name exactly the attempts this Run charged for the
	/// decision, each with its durable status and charged inputs, and carry
	/// exactly the answers those attempts durably recorded.
	async fn check_attempts(
		&self,
		tx: &mut native::Transaction,
		evidence: &Evidence,
	) -> Result<()> {
		let charged: BTreeMap<Uuid, RecoveredAttempt> = attempts_of(tx, self.run.id, evidence.id)
			.await?
			.iter()
			.map(|row| recovered(row).map(|prior| (prior.permit.record.attempt, prior)))
			.collect::<Result<_>>()?;
		let named: BTreeSet<_> = evidence.attempts.iter().map(|attempt| attempt.id).collect();
		if named.len() != evidence.attempts.len() || !named.iter().eq(charged.keys()) {
			return Err(Error::Invalid(
				"decision evidence differs from its charged attempts".into(),
			));
		}
		let mut answered = BTreeMap::new();
		for attempt in &evidence.attempts {
			let saved = charged.get(&attempt.id).ok_or_else(|| {
				Error::Invalid("decision evidence names an uncharged attempt".into())
			})?;
			let record = &saved.permit.record;
			if record.request_digest != attempt.request_digest
				|| record.questions != attempt.questions
				|| record
					.questions
					.iter()
					.any(|question| !evidence.questions.contains_key(question))
				|| saved.permit.owner_receipts != attempt.owner_receipts
				|| !same_input(&record.boundary, &evidence.boundary)
				|| record.decider != evidence.decider
				|| record.state_digest != evidence.state_digest
				|| record.mode != evidence.mode
				|| !at_least_as_strict(&evidence.restrictions, &record.binding_restrictions)
				|| attempt.status != saved.status
			{
				return Err(Error::Invalid(
					"decision evidence differs from its charged attempts".into(),
				));
			}
			answered.extend(saved.answers.iter().flatten().map(|(q, a)| (q.clone(), *a)));
		}
		// Charged plans never overlap, so the union is exact; an answer no
		// answered attempt recorded is never evidence of a provider call.
		if evidence.answers != answered {
			return Err(Error::Invalid(
				"decision evidence differs from its charged attempts".into(),
			));
		}
		Ok(())
	}

	async fn commit_evidence(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> Result<()> {
		self.check_boundary(&evidence.boundary)?;
		if evidence.decider != self.allowance.decider {
			return Err(Error::Invalid(
				"decision evidence names another Decider than the Run's admitted pin".into(),
			));
		}
		if context.is_some() != (evidence.outcome == Outcome::Applied) {
			return Err(Error::Invalid(
				"only an applied decision replaces the Run's context".into(),
			));
		}
		// A rejection after an early failure has nothing to replay, but it still
		// keeps every invariant a rejection has at any stage.
		if evidence.outcome == Outcome::Rejected {
			evidence.validate_rejection()?;
		} else {
			evidence.replay()?;
		}
		// A rejection skips replay, yet later recovery and outcomes still link
		// to its stable decision identity and state commitment.
		if evidence.id.is_nil() {
			return Err(Error::Invalid(
				"decision evidence lacks its decision identity".into(),
			));
		}
		validate_digest(&evidence.state_digest)?;
		match (state, &evidence.state, &evidence.state_retention) {
			(
				Some(state),
				StateReference::Retained {
					id,
					digest,
					expires_at,
				},
				StateRetentionWitness::Enabled {
					id: witness,
					expires_at: deadline,
				},
			) if state.id == *id
				&& state.id == *witness
				&& state.digest == *digest
				&& state.digest == evidence.state_digest
				&& rules::digest(&state.state) == state.digest
				&& state.expires_at == *expires_at
				&& state.expires_at == *deadline => {}
			(
				None,
				StateReference::Expired {
					id,
					digest,
					expired_at,
				},
				StateRetentionWitness::Enabled {
					id: witness,
					expires_at: deadline,
				},
			) if !id.is_nil()
				&& id == witness
				&& *digest == evidence.state_digest
				&& expired_at == deadline
				&& *expired_at <= self.now() => {}
			(None, StateReference::Disabled, StateRetentionWitness::Disabled) => {}
			_ => {
				return Err(Error::Invalid(
					"retained decision state differs from its evidence".into(),
				));
			}
		}
		let mut stored = evidence.clone();
		let mut tx = native::begin(&self.store.pool).await?;
		fence(&mut tx, &evidence.boundary).await?;
		ensure_open(&mut tx, self.run.id, evidence.id).await?;
		self.check_attempts(&mut tx, evidence).await?;
		// State whose deadline passed during dispatch keeps only its expired
		// marker. This is decided again right before the write, because the
		// policy lookup and the Run save below can outlast the deadline.
		let expire_elapsed = |retained: &mut Option<&RetainedState>, stored: &mut Evidence| {
			if let Some(state) = retained.take_if(|state| state.expires_at <= self.now()) {
				stored.state = StateReference::Expired {
					id: state.id,
					digest: state.digest.clone(),
					expired_at: state.expires_at,
				};
			}
		};
		let mut retained = state;
		expire_elapsed(&mut retained, &mut stored);
		// Applying context or retaining state needs the admitted pin to be current;
		// evidence without either effect is still recorded after a revocation.
		if (context.is_some() || retained.is_some())
			&& self
				.current
				.current(&mut tx, &self.run)
				.await?
				.is_none_or(|current| current.decider != self.allowance.decider)
		{
			return Err(Error::Forbidden);
		}
		let mut closed = Vec::new();
		if let Some(context) = context {
			let mut run = self.run.clone();
			run.context = context.clone();
			let (_, interrupted) = self
				.store
				.save_run_in(&mut tx, &run, evidence.boundary.worker, "context.compacted")
				.await?;
			closed = interrupted;
		}
		expire_elapsed(&mut retained, &mut stored);
		let evidence_id = Uuid::new_v4();
		native::query(
			&Query::insert()
				.into_table(Alias::new(EVIDENCE))
				.columns(
					[
						"id",
						"decision_id",
						"run_id",
						"outcome",
						"reason",
						"evidence",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::value(evidence_id),
					Expr::value(stored.id),
					Expr::value(self.run.id),
					Expr::value(label(&stored.outcome)?),
					Expr::value(label(&stored.reason)?),
					Expr::value(serde_json::to_value(&stored)?),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		if let Some(state) = retained {
			native::query(
				&Query::insert()
					.into_table(Alias::new(STATES))
					.columns(
						[
							"id",
							"evidence_id",
							"run_id",
							"digest",
							"expires_at",
							"state",
						]
						.map(Alias::new),
					)
					.values_panic([
						Expr::value(state.id),
						Expr::value(evidence_id),
						Expr::value(self.run.id),
						Expr::value(state.digest.as_str()),
						Expr::value(state.expires_at),
						Expr::value(state.state.clone()),
					])
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		super::inference::record_interruptions(&closed);
		Ok(())
	}

	async fn append(&self, outcome: &OutcomeLink) -> Result<()> {
		let bounded = |value: &str| (1..=256).contains(&value.chars().count());
		if !bounded(&outcome.observed_status)
			|| outcome
				.next_inference
				.as_deref()
				.is_some_and(|v| !bounded(v))
		{
			return Err(Error::Invalid("decision outcome is out of bounds".into()));
		}
		let mut tx = native::begin(&self.store.pool).await?;
		let decided: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(EVIDENCE))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.and_where(Expr::col("decision_id").eq(Expr::value(outcome.decision)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if decided.is_none() {
			return Err(Error::NotFound(
				"decision has no evidence in this Run".into(),
			));
		}
		native::query(
			&Query::insert()
				.into_table(Alias::new(OUTCOMES))
				.columns(
					[
						"id",
						"decision_id",
						"run_id",
						"next_inference",
						"observed_status",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::value(Uuid::new_v4()),
					Expr::value(outcome.decision),
					Expr::value(self.run.id),
					Expr::value(outcome.next_inference.clone()),
					Expr::value(outcome.observed_status.as_str()),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await
	}
}

impl Store {
	/// Delete retained classification state whose deadline is at or before `now`,
	/// leaving the matching expired marker in its evidence in the same transaction.
	pub async fn expire_decision_states(&self, now: DateTime<Utc>) -> Result<usize> {
		let mut tx = native::begin(&self.pool).await?;
		let due = native::query(
			&Query::select()
				.columns(["id", "evidence_id", "run_id", "digest"].map(Alias::new))
				.from(Alias::new(STATES))
				.and_where(Expr::col("expires_at").lte(Expr::value(now)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut *tx)
		.await?;
		for row in &due {
			let id: Uuid = row.try_get("id")?;
			let evidence_id: Uuid = row.try_get("evidence_id")?;
			let run: Uuid = row.try_get("run_id")?;
			let digest: String = row.try_get("digest")?;
			let saved: Value = native::query_scalar(
				&Query::select()
					.column(Alias::new("evidence"))
					.from(Alias::new(EVIDENCE))
					.and_where(Expr::col("id").eq(Expr::value(evidence_id)))
					.and_where(Expr::col("run_id").eq(Expr::value(run)))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			let mut evidence: Evidence = serde_json::from_value(saved)?;
			// The column keeps PostgreSQL's microsecond precision, while the
			// evidence keeps the exact deadline its retention witness names.
			let expires_at = match &evidence.state {
				StateReference::Retained {
					id: retained,
					digest: retained_digest,
					expires_at,
				} if *retained == id && *retained_digest == digest => *expires_at,
				_ => {
					return Err(Error::Invalid(
						"retained decision state differs from its evidence".into(),
					));
				}
			};
			evidence.state = StateReference::Expired {
				id,
				digest,
				expired_at: expires_at,
			};
			native::query(
				&Query::update()
					.table(Alias::new(EVIDENCE))
					.value(Alias::new("evidence"), serde_json::to_value(&evidence)?)
					.and_where(Expr::col("id").eq(Expr::value(evidence_id)))
					.and_where(Expr::col("run_id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			native::query(
				&Query::delete()
					.from_table(Alias::new(STATES))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		Ok(due.len())
	}
}

#[async_trait::async_trait]
impl DecisionJournal for RunDecisionJournal<'_> {
	async fn recover(
		&self,
		decision: Uuid,
		boundary: &Boundary,
	) -> aidash_application::Result<Vec<RecoveredAttempt>> {
		Ok(self.recover_attempts(decision, boundary).await?)
	}
	async fn reserve(&self, record: &DispatchRecord) -> aidash_application::Result<Reservation> {
		Ok(self.reserve_attempt(record).await?)
	}
	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> aidash_application::Result<()> {
		Ok(self.finish(permit, answers, status).await?)
	}
	async fn commit(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> aidash_application::Result<()> {
		Ok(self.commit_evidence(evidence, context, state).await?)
	}
	/// Retention cleanup is Deployment-wide; it is not limited to this Run.
	async fn expire_states(&self, now: DateTime<Utc>) -> aidash_application::Result<usize> {
		Ok(self.store.expire_decision_states(now).await?)
	}
	async fn append_outcome(&self, outcome: &OutcomeLink) -> aidash_application::Result<()> {
		Ok(self.append(outcome).await?)
	}
}

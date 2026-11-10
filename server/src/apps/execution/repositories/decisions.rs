//! Lease-fenced persistence of Decision Evidence and Decision Attempts behind the
//! portable `DecisionGate`.
//!
//! One journal serves one leased Run and the exact Decider pin admitted for it.
//! Its boundary maps `run_revision` to `runs.revision` and `input_revision` to
//! `runs.observed_input_seq`; both must still match whenever the journal reserves,
//! recovers or commits. Each Decision Attempt counts against the Decider's
//! per-Run call limit.
use crate::{Error, Result, database::native, store::Store};
use aidash_application::ports::decision::{
	DecisionJournal, DispatchPermit, DispatchRecord, RecoveredAttempt, Reservation,
};
use aidash_domain::{
	Run,
	context::recovery::Failure,
	decision::{
		ApprovalEvidence, AttemptEvidence, AttemptProviderReport, AttemptStatus, Boundary,
		DeciderPin, Mode, Probability, Questions, Restrictions, approval_questions,
		policy::ExecutionAllowance, validate_digest,
	},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

mod authority;
pub(crate) use authority::{CatalogAuthority, ChainAllowance, require_approved};

const ATTEMPTS: &str = "decision_attempts";
const EVIDENCE: &str = "decision_evidence";
const PENDING: &str = "pending";
const ATTEMPT_COLUMNS: [&str; 23] = [
	"id",
	"decision_id",
	"run_id",
	"node",
	"step",
	"cycle",
	"retry",
	"input_revision",
	"input_digest",
	"worker",
	"run_revision",
	"decider",
	"binding_restrictions",
	"route",
	"request_digest",
	"state_digest",
	"questions",
	"question_digest",
	"mode",
	"owner_receipts",
	"status",
	"answers",
	"provider_report",
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
	let cycle: i32 = row.try_get("cycle")?;
	let retry: i32 = row.try_get("retry")?;
	let route: i32 = row.try_get("route")?;
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
			cycle: u32::try_from(cycle)
				.map_err(|_| Error::Invalid("negative decision cycle".into()))?,
			retry: u32::try_from(retry)
				.map_err(|_| Error::Invalid("negative decision retry".into()))?,
		},
		decider: serde_json::from_value(row.try_get("decider")?)?,
		binding_restrictions: serde_json::from_value::<Restrictions>(
			row.try_get("binding_restrictions")?,
		)?,
		route: u32::try_from(route).map_err(|_| Error::Invalid("negative Decider Route".into()))?,
		request_digest: row.try_get("request_digest")?,
		state_digest: row.try_get("state_digest")?,
		questions,
		question_digest: row.try_get("question_digest")?,
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
	let provider_report: Option<Value> = row.try_get("provider_report")?;
	let provider_report: AttemptProviderReport = provider_report
		.map(serde_json::from_value)
		.transpose()?
		.unwrap_or_default();
	provider_report.validate()?;
	Ok(RecoveredAttempt {
		provider_report,
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
			question_digest: requested.question_digest.clone(),
			..saved.clone()
		},
		requested,
	)
}

/// Lock the Run while `boundary.worker` holds its lease and its revisions match.
/// `FOR NO KEY UPDATE` still orders every write of the Run, yet lets the owners
/// charged during a reservation insert rows that reference it.
async fn fence(tx: &mut native::Transaction, boundary: &Boundary) -> Result<()> {
	let fenced = native::query(
		&Query::select()
			.columns(["revision", "observed_input_seq"].map(Alias::new))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(boundary.run)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(boundary.worker)))
			.and_where(Expr::col("lease_until").gt(Expr::current_timestamp()))
			.lock(LockType::NoKeyUpdate)
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

/// The worker's lease on the Run is live by the clock now. `fence` compares
/// with `CURRENT_TIMESTAMP`, which stays at the transaction's start.
async fn lease_held_now(tx: &mut native::Transaction, boundary: &Boundary) -> Result<()> {
	native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(boundary.run)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(boundary.worker)))
			.and_where(Expr::col("lease_until").gt(SimpleExpr::FunctionCall(
				"clock_timestamp".into_iden(),
				vec![],
			)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?
	.map(|_| ())
	.ok_or_else(|| Error::Conflict("worker lease lost".into()))
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

#[async_trait::async_trait]
impl<T: CurrentAllowance + ?Sized> CurrentAllowance for &T {
	async fn current(
		&self,
		tx: &mut native::Transaction,
		run: &Run,
	) -> Result<Option<ExecutionAllowance>> {
		(**self).current(tx, run).await
	}
}

/// What the evidence of one decision says about its charged attempts, whichever
/// hook the decision served.
struct Attested<'a> {
	/// The decision the attempts were charged under.
	decision: Uuid,
	boundary: &'a Boundary,
	decider: &'a DeciderPin,
	binding_restrictions: &'a Restrictions,
	restrictions: &'a Restrictions,
	state_digest: &'a str,
	mode: Mode,
	/// The question definitions the attempts asked.
	questions: &'a Questions,
	attempts: &'a [AttemptEvidence],
}

/// Decision Evidence and Decision Attempts of one leased Run and its admitted Decider.
pub struct RunDecisionJournal<'a> {
	store: &'a Store,
	run: Run,
	allowance: ExecutionAllowance,
	current: Box<dyn CurrentAllowance + 'a>,
}

impl<'a> RunDecisionJournal<'a> {
	/// `run` is the leased Run whose context an applied decision replaces;
	/// `allowance` names the admitted Decider pin and its per-Run call limit,
	/// and `current` supplies the policy each charge is checked against again.
	pub fn new(
		store: &'a Store,
		run: Run,
		allowance: ExecutionAllowance,
		current: impl CurrentAllowance + 'a,
	) -> Result<Self> {
		allowance.decider.validate()?;
		Ok(Self {
			store,
			run,
			allowance,
			current: Box::new(current),
		})
	}

	/// The receipt owner for this Run's own call limit.
	fn owner(&self) -> String {
		format!("{}/runs/{}", self.store.deployment, self.run.id)
	}

	/// A transaction in the Run's Tenant (#263).
	async fn begin(&self) -> Result<native::Transaction> {
		let scope = self.store.run_tenancy(self.run.id).await?;
		self.store.pool.begin(&scope).await
	}

	fn check_boundary(&self, boundary: &Boundary) -> Result<()> {
		boundary.validate()?;
		if boundary.run != self.run.id || boundary.node != self.store.deployment {
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
		validate_digest(&record.question_digest)?;
		if record.attempt.is_nil()
			|| record.decision.is_nil()
			|| record.questions.is_empty()
			|| unique.len() != record.questions.len()
			|| record.questions.iter().any(String::is_empty)
			|| record.binding_restrictions.validate_floor().is_err()
		{
			return Err(Error::Invalid("malformed Decision Attempt".into()));
		}
		// The attempt is keyed by its stable identity, so a retry under the
		// canonical key always finds the charge.
		if record.attempt
			!= aidash_application::decision::attempt_identity(
				record.decision,
				&record.request_digest,
				&record.questions,
			) {
			return Err(Error::Invalid(
				"Decision Attempt differs from its stable identity".into(),
			));
		}
		Ok(())
	}

	async fn recover_attempts(
		&self,
		decision: Uuid,
		boundary: &Boundary,
	) -> Result<Vec<RecoveredAttempt>> {
		self.check_boundary(boundary)?;
		let mut tx = self.begin().await?;
		fence(&mut tx, boundary).await?;
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
		let mut tx = self.begin().await?;
		fence(&mut tx, &record.boundary).await?;
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
		// A Decider the current policy no longer allows, or whose call limit is
		// spent, pauses the Run with its own reason.
		let Some(current) = self.current.current(&mut tx, &self.run).await? else {
			return Err(Error::Context(Failure::DeciderUnapproved));
		};
		if self.allowance.remaining(&current, consumed)? == 0 {
			return Err(Error::Context(Failure::DeciderCallsExhausted));
		}
		let owner_receipts = BTreeMap::from([(self.owner(), Uuid::new_v4())]);
		native::query(
			&Query::insert()
				.into_table(Alias::new(ATTEMPTS))
				.columns(
					ATTEMPT_COLUMNS[..21]
						.iter()
						.copied()
						.chain(["tenant"])
						.map(Alias::new),
				)
				.values_expr(vec![
					Expr::value(record.attempt).into(),
					Expr::value(record.decision).into(),
					Expr::value(record.boundary.run).into(),
					Expr::value(record.boundary.node.as_str()).into(),
					Expr::value(record.boundary.step).into(),
					Expr::value(
						i32::try_from(record.boundary.cycle)
							.map_err(|_| Error::Invalid("decision cycle overflow".into()))?,
					)
					.into(),
					Expr::value(
						i32::try_from(record.boundary.retry)
							.map_err(|_| Error::Invalid("decision retry overflow".into()))?,
					)
					.into(),
					Expr::value(record.boundary.input_revision).into(),
					Expr::value(record.boundary.input_digest.as_str()).into(),
					Expr::value(record.boundary.worker).into(),
					Expr::value(record.boundary.run_revision).into(),
					Expr::value(decider).into(),
					Expr::value(serde_json::to_value(&record.binding_restrictions)?).into(),
					Expr::value(
						i32::try_from(record.route)
							.map_err(|_| Error::Invalid("Decider Route overflow".into()))?,
					)
					.into(),
					Expr::value(record.request_digest.as_str()).into(),
					Expr::value(record.state_digest.as_str()).into(),
					Expr::value(serde_json::to_value(&record.questions)?).into(),
					Expr::value(record.question_digest.as_str()).into(),
					Expr::value(label(&record.mode)?).into(),
					Expr::value(serde_json::to_value(&owner_receipts)?).into(),
					Expr::value(PENDING).into(),
					crate::database::scope::tenant_of("runs", "id", record.boundary.run),
				])
				.expect("one value per column")
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		// The scans above can outlast the lease that `fence` saw, and no
		// successor can claim the Run while this transaction holds its row.
		lease_held_now(&mut tx, &record.boundary).await?;
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
		provider_report: &AttemptProviderReport,
	) -> Result<()> {
		provider_report.validate()?;
		if status == AttemptStatus::NotDispatched
			&& *provider_report != AttemptProviderReport::Unknown
		{
			return Err(Error::Invalid(
				"undispatched attempt cannot report provider facts".into(),
			));
		}
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
		let mut tx = self.begin().await?;
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
			if saved_status != status
				|| saved_answers != answers
				|| recovered(&saved)?.provider_report != *provider_report
			{
				return Err(Error::Conflict(
					"Decision Attempt already finished differently".into(),
				));
			}
			tx.commit().await?;
			return Ok(());
		}
		native::query(
			&Query::update()
				.table(Alias::new(ATTEMPTS))
				.value(Alias::new("status"), status.as_str())
				.value(Alias::new("answers"), answers)
				.value(
					Alias::new("provider_report"),
					serde_json::to_value(provider_report)?,
				)
				.value_expr(Alias::new("finished_at"), Expr::current_timestamp())
				.and_where(Expr::col("id").eq(Expr::value(record.attempt)))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}

	/// The attempts an approval decision's evidence names must be those its
	/// Route's decision charged, and its answers those they recorded. A
	/// decision whose Routes were all unavailable names the attempts of the last
	/// Route asked, or none.
	async fn check_approval_attempts(
		&self,
		tx: &mut native::Transaction,
		evidence: &ApprovalEvidence,
	) -> Result<()> {
		let Some(route) = evidence
			.route
			.or_else(|| evidence.failed_routes.last().copied())
		else {
			return if evidence.attempts.is_empty() {
				Ok(())
			} else {
				Err(Error::Invalid(
					"approval evidence names attempts of no Route".into(),
				))
			};
		};
		// An approval decision has no sources and no window.
		let decision = aidash_application::decision::decision_identity(
			&evidence.boundary,
			&evidence.decider,
			&evidence.binding_restrictions,
			&evidence.state_digest,
			&[],
			0,
			route,
		);
		let answered = self
			.charged_answers(
				tx,
				&Attested {
					decision,
					boundary: &evidence.boundary,
					decider: &evidence.decider,
					binding_restrictions: &evidence.binding_restrictions,
					restrictions: &evidence.restrictions,
					state_digest: &evidence.state_digest,
					mode: evidence.mode,
					questions: &approval_questions(),
					attempts: &evidence.attempts,
				},
			)
			.await?;
		if evidence.answers != answered {
			return Err(Error::Invalid(
				"approval evidence differs from its charged attempts".into(),
			));
		}
		Ok(())
	}

	/// The answers the attempts of `attested.decision` durably recorded, once the
	/// evidence is shown to name exactly the attempts this Run charged for it.
	async fn charged_answers(
		&self,
		tx: &mut native::Transaction,
		attested: &Attested<'_>,
	) -> Result<BTreeMap<String, Probability>> {
		let charged: BTreeMap<Uuid, RecoveredAttempt> =
			attempts_of(tx, self.run.id, attested.decision)
				.await?
				.iter()
				.map(|row| recovered(row).map(|prior| (prior.permit.record.attempt, prior)))
				.collect::<Result<_>>()?;
		let named: BTreeSet<_> = attested.attempts.iter().map(|attempt| attempt.id).collect();
		if named.len() != attested.attempts.len() || !named.iter().eq(charged.keys()) {
			return Err(Error::Invalid(
				"decision evidence differs from its charged attempts".into(),
			));
		}
		let mut answered = BTreeMap::new();
		for attempt in attested.attempts {
			let saved = charged.get(&attempt.id).ok_or_else(|| {
				Error::Invalid("decision evidence names an uncharged attempt".into())
			})?;
			let record = &saved.permit.record;
			if record.request_digest != attempt.request_digest
				|| record.questions != attempt.questions
				|| record
					.questions
					.iter()
					.any(|question| !attested.questions.contains_key(question))
				|| record.question_digest != attempt.question_digest
				// The charge committed to the definitions the request asked.
				|| aidash_domain::decision::question_digest(attested.questions, &record.questions)
					.ok()
					.as_ref()
					!= Some(&record.question_digest)
				|| &record.binding_restrictions != attested.binding_restrictions
				|| saved.permit.owner_receipts != attempt.owner_receipts
				|| !same_input(&record.boundary, attested.boundary)
				|| &record.decider != attested.decider
				|| record.state_digest != attested.state_digest
				|| record.mode != attested.mode
				|| !attested
					.restrictions
					.at_least_as_strict(&record.binding_restrictions)
				|| attempt.status != saved.status
                || attempt.provider_report != saved.provider_report
			{
				return Err(Error::Invalid(
					"decision evidence differs from its charged attempts".into(),
				));
			}
			answered.extend(saved.answers.iter().flatten().map(|(q, a)| (q.clone(), *a)));
		}
		Ok(answered)
	}

	/// Record one approval decision (ADR 0077). Its identity commits to the
	/// approval, the Decider, the Route and the state, so judging again after a
	/// crash, under another worker, reproduces it: evidence already recorded under
	/// that identity is kept, whatever worker judged it.
	async fn commit_approval_evidence(&self, evidence: &ApprovalEvidence) -> Result<()> {
		self.check_boundary(&evidence.boundary)?;
		if evidence.decider != self.allowance.decider {
			return Err(Error::Invalid(
				"approval evidence names another Decider than the Run's admitted pin".into(),
			));
		}
		evidence.validate()?;
		if aidash_application::decision::approval_evidence_identity(
			&evidence.boundary.node,
			evidence.boundary.run,
			evidence.approval,
			&evidence.decider,
			evidence.route,
			&evidence.state_digest,
		) != evidence.id
		{
			return Err(Error::Invalid(
				"approval evidence differs from its decision identity".into(),
			));
		}
		let mut tx = self.begin().await?;
		fence(&mut tx, &evidence.boundary).await?;
		let recorded: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(EVIDENCE))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run.id)))
				.and_where(Expr::col("decision_id").eq(Expr::value(evidence.id)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if recorded.is_some() {
			tx.commit().await?;
			return Ok(());
		}
		self.check_approval_attempts(&mut tx, evidence).await?;
		// The checks above can outlast the lease that `fence` saw.
		lease_held_now(&mut tx, &evidence.boundary).await?;
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
						"tenant",
					]
					.map(Alias::new),
				)
				.values_expr(vec![
					Expr::value(Uuid::new_v4()).into(),
					Expr::value(evidence.id).into(),
					Expr::value(self.run.id).into(),
					Expr::value(label(&evidence.outcome())?).into(),
					Expr::value(evidence.reason()).into(),
					Expr::value(serde_json::to_value(evidence)?).into(),
					crate::database::scope::tenant_of("runs", "id", self.run.id),
				])
				.expect("one value per column")
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await
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
		Ok(self
			.finish(permit, answers, status, &AttemptProviderReport::Unknown)
			.await?)
	}
	async fn finish_reported_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
		provider_report: &AttemptProviderReport,
	) -> aidash_application::Result<()> {
		Ok(self
			.finish(permit, answers, status, provider_report)
			.await?)
	}
	async fn commit_approval(&self, evidence: &ApprovalEvidence) -> aidash_application::Result<()> {
		Ok(self.commit_approval_evidence(evidence).await?)
	}
}

//! Lease-fenced persistence of Decision Evidence and Decision Attempts behind the
//! portable `DecisionGate`.
//!
//! One journal serves one leased Run and the exact Decider pin admitted for it.
//! Its boundary maps `run_revision` to `runs.revision` and `input_revision` to
//! `runs.observed_input_seq`; both must still match whenever the journal reserves,
//! recovers or commits. Each Decision Attempt counts against the Decider's
//! per-Run call limit, and against the `DecisionOwners` of a generated chain or
//! RequiredHome execution. A staged
//! decision is recorded in the transaction that adopts its summary (ADR 0056).
use crate::{Error, Result, database::native, store::Store};
use aidash_application::decision::StagedDecision;
use aidash_application::ports::decision::{
	DecisionJournal, DispatchPermit, DispatchRecord, RecoveredAttempt, Reservation, RetainedState,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::{
	Run,
	context::{Context, recovery::Failure},
	decision::{
		ApprovalEvidence, AttemptEvidence, AttemptProviderReport, AttemptStatus, Boundary,
		DeciderPin, Evidence, Mode, Outcome, OutcomeLink, Probability, Questions, Restrictions,
		StateReference, StateRetentionWitness, approval_questions, policy::ExecutionAllowance,
		pruning, validate_digest,
	},
	registry::rules,
};
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

mod authority;
pub(crate) use authority::{
	AdmittedAllowance, CatalogAuthority, ChainAllowance, ChainOwners, require_approved,
};

const ATTEMPTS: &str = "decision_attempts";
const EVIDENCE: &str = "decision_evidence";
const STATES: &str = "decision_states";
const OUTCOMES: &str = "decision_outcomes";
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

/// A summary removes the entries it absorbs from the projection it continues,
/// so the adopted history keeps the pruned entries it did not absorb, in order.
fn continues_from(
	adopted: &[aidash_domain::context::HistoryEntry],
	pruned: &[aidash_domain::context::HistoryEntry],
) -> bool {
	let mut remaining = pruned.iter();
	adopted
		.iter()
		.all(|entry| remaining.any(|candidate| candidate == entry))
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

/// An applied, staged or shadow decision is final and is never overwritten.
async fn ensure_open(tx: &mut native::Transaction, run: Uuid, decision: Uuid) -> Result<()> {
	let finished: Option<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(EVIDENCE))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("decision_id").eq(Expr::value(decision)))
			.and_where(Expr::col("outcome").is_in(["applied", "staged", "shadow"]))
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

/// The owners beside the Run itself that a Decision Attempt is charged to: every
/// generated ancestor, or the Home of a RequiredHome execution.
#[async_trait::async_trait]
pub trait DecisionOwners: Send + Sync {
	/// Charge one Decision Attempt and return each owner's receipt. A spent
	/// budget is `decider_calls_exhausted`; nothing is dispatched after an error.
	async fn charge(&self, record: &DispatchRecord) -> Result<BTreeMap<String, Uuid>>;
	/// Release what `charge` kept open once the attempt's result is recorded;
	/// the charge itself is kept.
	async fn settle(&self, _record: &DispatchRecord) -> Result<()> {
		Ok(())
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
	owners: Option<Box<dyn DecisionOwners + 'a>>,
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
			owners: None,
		})
	}

	/// Charge `owners` too before each Decision Attempt is dispatched.
	pub fn with_owners(mut self, owners: impl DecisionOwners + 'a) -> Self {
		self.owners = Some(Box::new(owners));
		self
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
		let mut tx = self.begin().await?;
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
		// A Decider the current policy no longer allows, or whose call limit is
		// spent, pauses the Run with its own reason.
		let Some(current) = self.current.current(&mut tx, &self.run).await? else {
			return Err(Error::Context(Failure::DeciderUnapproved));
		};
		if self.allowance.remaining(&current, consumed)? == 0 {
			return Err(Error::Context(Failure::DeciderCallsExhausted));
		}
		let mut owner_receipts = BTreeMap::from([(self.owner(), Uuid::new_v4())]);
		// Every other owner is charged before the attempt is recorded; a spent
		// or withdrawn owner leaves nothing to dispatch.
		if let Some(owners) = &self.owners {
			owner_receipts.extend(owners.charge(record).await?);
		}
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
			// A settlement that failed after the status committed is retried.
			if let Some(owners) = &self.owners {
				owners.settle(record).await?;
			}
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
		// The other owners' reservations close once the result is kept.
		if let Some(owners) = &self.owners {
			owners.settle(record).await?;
		}
		Ok(())
	}

	/// The evidence must name exactly the attempts this Run charged for the
	/// decision, each with its durable status and charged inputs, and carry
	/// exactly the answers those attempts durably recorded.
	async fn check_attempts(
		&self,
		tx: &mut native::Transaction,
		evidence: &Evidence,
	) -> Result<()> {
		let answered = self
			.charged_answers(
				tx,
				&Attested {
					decision: evidence.id,
					boundary: &evidence.boundary,
					decider: &evidence.decider,
					binding_restrictions: &evidence.binding_restrictions,
					restrictions: &evidence.restrictions,
					state_digest: &evidence.state_digest,
					mode: evidence.mode,
					questions: &evidence.questions,
					attempts: &evidence.attempts,
				},
			)
			.await?;
		// The decision UUID commits to its inputs and the admitted Binding
		// restrictions the evidence keeps, so evidence that names other sources
		// or another window under it is not this decision, attempts or not.
		if aidash_application::decision::decision_identity(
			&evidence.boundary,
			&evidence.decider,
			&evidence.binding_restrictions,
			&evidence.state_digest,
			&evidence.sources,
			evidence.fit.window,
			evidence.route,
		) != evidence.id
		{
			return Err(Error::Invalid(
				"decision evidence differs from its decision identity".into(),
			));
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

	/// Checks that need no transaction: the boundary and pin, the evidence's
	/// own invariants and its retained-state witness.
	fn check_evidence(&self, evidence: &Evidence, state: Option<&RetainedState>) -> Result<()> {
		self.check_boundary(&evidence.boundary)?;
		if evidence.decider != self.allowance.decider {
			return Err(Error::Invalid(
				"decision evidence names another Decider than the Run's admitted pin".into(),
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
		Ok(())
	}

	/// State whose deadline passed keeps only its expired marker.
	fn expire_elapsed(&self, retained: &mut Option<&RetainedState>, stored: &mut Evidence) {
		if let Some(state) = retained.take_if(|state| state.expires_at <= self.now()) {
			stored.state = StateReference::Expired {
				id: state.id,
				digest: state.digest.clone(),
				expired_at: state.expires_at,
			};
		}
	}

	/// Inside the fenced commit transaction: the decision is still open, the
	/// evidence names exactly its charged attempts, and an effect on the Run
	/// or retained state needs the admitted pin to be current. Evidence
	/// without either effect is still recorded after a revocation.
	async fn prepare_in<'s>(
		&self,
		tx: &mut native::Transaction,
		evidence: &Evidence,
		changes_run: bool,
		state: Option<&'s RetainedState>,
	) -> Result<(Evidence, Option<&'s RetainedState>)> {
		ensure_open(tx, self.run.id, evidence.id).await?;
		self.check_attempts(tx, evidence).await?;
		let mut stored = evidence.clone();
		let mut retained = state;
		self.expire_elapsed(&mut retained, &mut stored);
		if (changes_run || retained.is_some())
			&& self
				.current
				.current(tx, &self.run)
				.await?
				.is_none_or(|current| current.decider != self.allowance.decider)
		{
			return Err(Error::Forbidden);
		}
		Ok((stored, retained))
	}

	/// Write the evidence and any retained state. The expiry is decided again
	/// right before the write, because the policy lookup and the Run save can
	/// outlast the deadline.
	async fn insert_in(
		&self,
		tx: &mut native::Transaction,
		mut stored: Evidence,
		mut retained: Option<&RetainedState>,
	) -> Result<()> {
		self.expire_elapsed(&mut retained, &mut stored);
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
						"tenant",
					]
					.map(Alias::new),
				)
				.values_expr(vec![
					Expr::value(evidence_id).into(),
					Expr::value(stored.id).into(),
					Expr::value(self.run.id).into(),
					Expr::value(label(&stored.outcome)?).into(),
					Expr::value(label(&stored.reason)?).into(),
					Expr::value(serde_json::to_value(&stored)?).into(),
					crate::database::scope::tenant_of("runs", "id", self.run.id),
				])
				.expect("one value per column")
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
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
							"tenant",
						]
						.map(Alias::new),
					)
					.values_expr(vec![
						Expr::value(state.id).into(),
						Expr::value(evidence_id).into(),
						Expr::value(self.run.id).into(),
						Expr::value(state.digest.as_str()).into(),
						Expr::value(state.expires_at).into(),
						Expr::value(state.state.clone()).into(),
						crate::database::scope::tenant_of("runs", "id", self.run.id),
					])
					.expect("one value per column")
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?;
			// State that expired during the awaits above would be kept under a
			// retained marker; refuse it, and a retry records the expired marker.
			let live: bool = native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(CAST(? AS timestamptz) > clock_timestamp())".to_owned(),
						vec![Expr::value(state.expires_at).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **tx)
			.await?;
			if !live {
				return Err(Error::Conflict(
					"retained decision state expired while it was written".into(),
				));
			}
		}
		Ok(())
	}

	async fn commit_evidence(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> Result<()> {
		if evidence.outcome == Outcome::Staged {
			return Err(Error::Invalid(
				"a staged decision is recorded with the summary that completes it".into(),
			));
		}
		if context.is_some() != (evidence.outcome == Outcome::Applied) {
			return Err(Error::Invalid(
				"only an applied decision replaces the Run's context".into(),
			));
		}
		self.check_evidence(evidence, state)?;
		// The saved projection must be the one the evidence attests to.
		if let Some(context) = context
			&& Some(pruning::projection_digest(&context.history)?) != evidence.projection_digest
		{
			return Err(Error::Invalid(
				"applied context differs from its decision evidence".into(),
			));
		}
		let mut tx = self.begin().await?;
		fence(&mut tx, &evidence.boundary).await?;
		let (stored, retained) = self
			.prepare_in(&mut tx, evidence, context.is_some(), state)
			.await?;
		// The checks above can outlast the lease that `fence` saw. Saving an
		// applied context ends the step and releases the lease, so this is
		// the last point at which the worker must still hold it.
		lease_held_now(&mut tx, &evidence.boundary).await?;
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
		self.insert_in(&mut tx, stored, retained).await?;
		tx.commit().await?;
		super::inference::record_interruptions(&closed);
		Ok(())
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

	/// The first half of recording a staged decision inside the transaction
	/// that adopts the summary completing its pruning (ADR 0056): the evidence
	/// names the pruned projection, and `adopted` continues from it.
	pub(crate) async fn prepare_staged_in<'s>(
		&self,
		tx: &mut native::Transaction,
		staged: &'s StagedDecision,
		adopted: &Context,
	) -> Result<(Evidence, Option<&'s RetainedState>)> {
		let evidence = &staged.evidence;
		if evidence.outcome != Outcome::Staged {
			return Err(Error::Invalid(
				"only a staged decision is recorded with a summary".into(),
			));
		}
		self.check_evidence(evidence, staged.state.as_ref())?;
		let pruned = &staged.pruned.history;
		if Some(pruning::projection_digest(pruned)?) != evidence.projection_digest
			|| !continues_from(&adopted.history, pruned)
		{
			return Err(Error::Invalid(
				"the adopted summary does not continue from the staged pruning".into(),
			));
		}
		fence(tx, &evidence.boundary).await?;
		self.prepare_in(tx, evidence, true, staged.state.as_ref())
			.await
	}

	/// The second half, after the adopted projection is saved.
	pub(crate) async fn insert_staged_in(
		&self,
		tx: &mut native::Transaction,
		(stored, retained): (Evidence, Option<&RetainedState>),
	) -> Result<()> {
		self.insert_in(tx, stored, retained).await
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
		let mut tx = self.begin().await?;
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
						"tenant",
					]
					.map(Alias::new),
				)
				.values_expr(vec![
					Expr::value(Uuid::new_v4()).into(),
					Expr::value(outcome.decision).into(),
					Expr::value(self.run.id).into(),
					Expr::value(outcome.next_inference.clone()).into(),
					Expr::value(outcome.observed_status.as_str()).into(),
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

impl Store {
	/// Delete retained classification state whose deadline is at or before `now`,
	/// leaving the matching expired marker in its evidence in the same transaction.
	pub async fn expire_decision_states(&self, now: DateTime<Utc>) -> Result<usize> {
		// Retention cleanup is Deployment-wide; it is not limited to one Run.
		let mut tx = self
			.pool
			.begin(&TenancyScope::Deployment(DeploymentScopeReason::Retention))
			.await?;
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
	async fn commit(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> aidash_application::Result<()> {
		Ok(self.commit_evidence(evidence, context, state).await?)
	}
	async fn commit_approval(&self, evidence: &ApprovalEvidence) -> aidash_application::Result<()> {
		Ok(self.commit_approval_evidence(evidence).await?)
	}
	/// Retention cleanup is Deployment-wide; it is not limited to this Run.
	async fn expire_states(&self, now: DateTime<Utc>) -> aidash_application::Result<usize> {
		Ok(self.store.expire_decision_states(now).await?)
	}
	async fn append_outcome(&self, outcome: &OutcomeLink) -> aidash_application::Result<()> {
		Ok(self.append(outcome).await?)
	}
}

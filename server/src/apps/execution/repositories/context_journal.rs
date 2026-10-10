//! Context Journal persistence: the append-only original events behind the
//! lossy `runs.context` projection, and lease-fenced Compaction Attempts.
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{
	Run,
	context::{
		Context, HistoryEntry,
		recovery::{Attempt, Failure, Outcome, Settlement},
	},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	UpdateStatement,
};
use uuid::Uuid;

const EVENTS: &str = "run_context_events";
const ATTEMPTS: &str = "context_compaction_attempts";

fn signed(value: u64) -> Result<i64> {
	i64::try_from(value).map_err(|_| Error::Invalid("context journal value is out of range".into()))
}

fn outcome(outcome: Outcome) -> &'static str {
	match outcome {
		Outcome::Adopted => "adopted",
		Outcome::Insufficient => "insufficient",
		Outcome::Unavailable => "unavailable",
		Outcome::Invalid => "invalid",
		Outcome::Unauthorized => "unauthorized",
		Outcome::Abandoned => "abandoned",
	}
}

/// Highest journaled sequence of `run`, or 0 before its first entry.
async fn head(tx: &mut native::Transaction, run: Uuid) -> Result<u64> {
	let head: i64 = native::query_scalar(
		&Query::select()
			.expr(Expr::cust("COALESCE(MAX(seq), 0)"))
			.from(Alias::new(EVENTS))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	u64::try_from(head).map_err(|_| Error::Invalid("context journal head is negative".into()))
}

/// Journal every projection entry newer than the persisted head. Runs in the
/// transaction that writes `runs.context`, after its lease-fenced row update.
pub(crate) async fn append(
	tx: &mut native::Transaction,
	run: Uuid,
	context: &Context,
) -> Result<()> {
	let persisted = head(tx, run).await?;
	let mut insert = Query::insert();
	insert
		.into_table(Alias::new(EVENTS))
		.columns(["run_id", "seq", "origin", "event", "digest"].map(Alias::new));
	let mut pending = false;
	for entry in context.unjournaled(persisted) {
		let event = serde_json::to_value(&entry.event)?;
		let digest = aidash_domain::registry::rules::digest(&event);
		let origin = if entry.seq <= context.journal.imported_through {
			"imported"
		} else {
			"appended"
		};
		insert.values_panic([
			Expr::value(run),
			Expr::value(signed(entry.seq)?),
			Expr::value(origin),
			Expr::value(event),
			Expr::value(digest),
		]);
		pending = true;
	}
	if pending {
		native::query(&insert.to_string(PostgresQueryBuilder))
			.execute(&mut **tx)
			.await?;
	}
	Ok(())
}

/// Settle every unsettled attempt of `run` as abandoned.
pub(crate) fn abandon_open(run: Uuid) -> UpdateStatement {
	Query::update()
		.table(Alias::new(ATTEMPTS))
		.value(Alias::new("outcome"), outcome(Outcome::Abandoned))
		.value_expr(Alias::new("settled_at"), Expr::current_timestamp())
		.and_where(Expr::col("run_id").eq(Expr::value(run)))
		.and_where(Expr::col("outcome").is_null())
		.to_owned()
}

/// Lock the Run row while `worker` still holds its lease.
async fn fence(tx: &mut native::Transaction, run: Uuid, worker: Uuid) -> Result<()> {
	let leased: Option<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.and_where(Expr::col("lease_owner").eq(Expr::value(worker)))
			.and_where(Expr::col("lease_until").gt(Expr::current_timestamp()))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **tx)
	.await?;
	leased
		.map(|_| ())
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))
}

/// Settle `attempt` only while it is open; returns whether it was.
async fn settle(
	tx: &mut native::Transaction,
	run: Uuid,
	attempt: Uuid,
	settlement: &Settlement,
) -> Result<bool> {
	let after_tokens = settlement.after_tokens.map(signed).transpose()?;
	let settled = native::query(
		&Query::update()
			.table(Alias::new(ATTEMPTS))
			.value(Alias::new("outcome"), outcome(settlement.outcome))
			.value(Alias::new("reason"), settlement.reason.clone())
			.value(
				Alias::new("candidate_digest"),
				settlement.candidate_digest.clone(),
			)
			.value(Alias::new("after_tokens"), after_tokens)
			.value_expr(Alias::new("settled_at"), Expr::current_timestamp())
			.and_where(Expr::col("id").eq(Expr::value(attempt)))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("outcome").is_null())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(settled.rows_affected() == 1)
}

impl Store {
	/// Original journal entries of `run` with `from <= seq <= through`, in order.
	pub(crate) async fn context_journal(
		&self,
		run: Uuid,
		from: u64,
		through: u64,
	) -> Result<Vec<HistoryEntry>> {
		native::query(
			&Query::select()
				.columns(["seq", "event"].map(Alias::new))
				.from(Alias::new(EVENTS))
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.and_where(Expr::col("seq").gte(Expr::value(signed(from)?)))
				.and_where(Expr::col("seq").lte(Expr::value(signed(through)?)))
				.order_by(Alias::new("seq"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?
		.into_iter()
		.map(|row| {
			let seq: i64 = row.try_get("seq")?;
			Ok(HistoryEntry {
				seq: u64::try_from(seq)
					.map_err(|_| Error::Invalid("context journal sequence is negative".into()))?,
				event: serde_json::from_value(row.try_get("event")?)?,
			})
		})
		.collect()
	}

	/// Journal saved projection entries the journal lacks, under the lease and
	/// without saving the Run, so imported history survives later pruning.
	pub(crate) async fn journal_context(&self, run: &Run, worker: Uuid) -> Result<()> {
		let mut tx = native::begin(&self.pool).await?;
		fence(&mut tx, run.id, worker).await?;
		append(&mut tx, run.id, &run.context).await?;
		tx.commit().await
	}

	/// Record a Compaction Attempt before provider I/O. Earlier unsettled
	/// attempts are abandoned even when the stage budget is already spent.
	pub(crate) async fn begin_compaction(
		&self,
		run: &Run,
		worker: Uuid,
		attempt: &Attempt,
		call_budget: u32,
	) -> Result<()> {
		if attempt.run_id != run.id {
			return Err(Error::Invalid(
				"compaction attempt belongs to another Run".into(),
			));
		}
		let mut tx = native::begin(&self.pool).await?;
		fence(&mut tx, run.id, worker).await?;
		native::query(&abandon_open(run.id).to_string(PostgresQueryBuilder))
			.execute(&mut *tx)
			.await?;
		let spent: i64 = native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new(ATTEMPTS))
				.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
				.and_where(Expr::col("stage").eq(Expr::value(attempt.stage.label())))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		if spent >= i64::from(call_budget) {
			tx.commit().await?;
			return Err(Error::Context(Failure::SummaryUnavailable));
		}
		native::query(
			&Query::insert()
				.into_table(Alias::new(ATTEMPTS))
				.columns(
					[
						"id",
						"run_id",
						"stage",
						"policy_version",
						"provider",
						"source_from_seq",
						"source_through_seq",
						"base_revision",
						"observed_input_seq",
						"before_tokens",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::value(attempt.id),
					Expr::value(run.id),
					Expr::value(attempt.stage.label()),
					Expr::value(attempt.policy_version.as_str()),
					Expr::value(attempt.provider.as_str()),
					Expr::value(signed(attempt.source_from_seq)?),
					Expr::value(signed(attempt.source_through_seq)?),
					Expr::value(attempt.base_revision),
					Expr::value(attempt.observed_input_seq),
					Expr::value(signed(attempt.before_tokens)?),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await
	}

	/// Settle an attempt that leaves the saved projection unchanged. An attempt
	/// already settled elsewhere, such as by lease recovery, stays as recorded.
	pub(crate) async fn settle_compaction(
		&self,
		run: &Run,
		worker: Uuid,
		attempt: Uuid,
		settlement: &Settlement,
	) -> Result<()> {
		if settlement.outcome == Outcome::Adopted {
			return Err(Error::Invalid(
				"an adopted attempt settles only with its Run save".into(),
			));
		}
		let mut tx = native::begin(&self.pool).await?;
		fence(&mut tx, run.id, worker).await?;
		settle(&mut tx, run.id, attempt, settlement).await?;
		tx.commit().await
	}

	/// Save the adopted projection and mark its attempt adopted atomically.
	pub(crate) async fn adopt_compaction(
		&self,
		run: &Run,
		worker: Uuid,
		attempt: Uuid,
		settlement: &Settlement,
	) -> Result<()> {
		if settlement.outcome != Outcome::Adopted {
			return Err(Error::Invalid(
				"only an adopted settlement changes the Context Projection".into(),
			));
		}
		let mut tx = native::begin(&self.pool).await?;
		// Lock the Run before the attempt, in the order lease recovery uses.
		fence(&mut tx, run.id, worker).await?;
		let through: Option<i64> = native::query_scalar(
			&Query::select()
				.column(Alias::new("source_through_seq"))
				.from(Alias::new(ATTEMPTS))
				.and_where(Expr::col("id").eq(Expr::value(attempt)))
				.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
				.and_where(Expr::col("outcome").is_null())
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		let through = through
			.ok_or_else(|| Error::Conflict("compaction attempt is no longer open".into()))?;
		self.save_run_in(&mut tx, run, worker, "context.compacted")
			.await?;
		if signed(head(&mut tx, run.id).await?)? < through {
			return Err(Error::Conflict(
				"context journal lacks the compacted source range".into(),
			));
		}
		if !settle(&mut tx, run.id, attempt, settlement).await? {
			return Err(Error::Conflict(
				"compaction attempt is no longer open".into(),
			));
		}
		tx.commit().await
	}
}

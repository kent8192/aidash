//! Authority Changes and the new work that waits for them (ADR 0121).
//!
//! An Authority Change returns only once nothing admitted under the old rules
//! is still running, so it waits on row locks that running work holds. A
//! waiting `FOR UPDATE` does not stop new `FOR SHARE` requests, so while work
//! overlaps it would never get its turn. So it raises a flag for its Tenant,
//! an exclusive advisory lock its transaction holds, and new work tests that
//! flag at its entrance. New work holds no advisory lock and no connection
//! while it waits, and code inside an open hold never tests, so nothing can
//! wait on itself.
use crate::{Error, Result, database::native::Pool};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use std::{
	collections::HashMap,
	sync::{LazyLock, Mutex, OnceLock},
	time::{Duration, Instant},
};

const SETTING: &str = "AIDASH_POLICY_WAIT_SECONDS";
const DEFAULT_LIMIT: Duration = Duration::from_secs(10);
/// How often waiting work tests the flag again.
const RECHECK: Duration = Duration::from_millis(200);
/// The refusal pause is this many limits, so at worst a Tenant's new work
/// waits one part in `1 + PAUSE_LIMITS` of the time.
const PAUSE_LIMITS: u32 = 3;
const FLAG_SEED: i64 = 71_003_768;
/// Transaction-local, so it ends with the flag it records, savepoints included.
const RAISED: &str = "aidash.authority_change_raised";

/// The longest wait of an Authority Change, for startup to report a bad value.
fn configured_limit() -> Result<Duration> {
	let seconds = match std::env::var(SETTING) {
		Ok(raw) => raw
			.parse::<u64>()
			.map_err(|_| Error::Invalid(format!("invalid {SETTING}")))?,
		Err(std::env::VarError::NotPresent) => return Ok(DEFAULT_LIMIT),
		Err(_) => return Err(Error::Invalid(format!("invalid {SETTING}"))),
	};
	if !(1..=3600).contains(&seconds) {
		return Err(Error::Invalid(format!("{SETTING} must be 1..=3600")));
	}
	Ok(Duration::from_secs(seconds))
}

fn limit() -> Duration {
	static LIMIT: OnceLock<Duration> = OnceLock::new();
	*LIMIT.get_or_init(|| configured_limit().unwrap_or(DEFAULT_LIMIT))
}

/// Refuses a limit a change could not answer within the request limit. A
/// second change waits behind the first, so its answer can take twice the limit.
pub(crate) fn validate(request_limit: Duration) -> Result<()> {
	if configured_limit()?.saturating_mul(2) >= request_limit {
		return Err(Error::Invalid(format!(
			"{SETTING} must be under half of AIDASH_HTTP_TIMEOUT_SECONDS"
		)));
	}
	Ok(())
}

fn pause_length() -> Duration {
	limit().saturating_mul(PAUSE_LIMITS)
}

/// When each Tenant's pause after a refusal ends, per process like the HTTP
/// limits. The database is part of the key because one process can serve several.
fn pauses() -> &'static Mutex<HashMap<(String, String), Instant>> {
	static PAUSES: LazyLock<Mutex<HashMap<(String, String), Instant>>> =
		LazyLock::new(Default::default);
	&PAUSES
}

fn seconds_up(duration: Duration) -> u64 {
	duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

fn paused(database: &str, tenant: &str) -> Option<u64> {
	let now = Instant::now();
	let mut pauses = pauses().lock().unwrap_or_else(|error| error.into_inner());
	let key = (database.to_owned(), tenant.to_owned());
	match pauses.get(&key) {
		Some(until) if *until > now => Some(seconds_up(*until - now)),
		Some(_) => {
			pauses.remove(&key);
			None
		}
		None => None,
	}
}

fn refuse(database: &str, tenant: &str, waited: Duration, oldest: Option<f64>) -> Error {
	let length = pause_length();
	let until = Instant::now() + length;
	let mut pauses = pauses().lock().unwrap_or_else(|error| error.into_inner());
	pauses.retain(|_, end| *end > Instant::now());
	pauses.insert((database.to_owned(), tenant.to_owned()), until);
	metrics::counter!("aidash_authority_change_refusals_total").increment(1);
	tracing::warn!(
		%tenant,
		waited_ms = waited.as_millis() as u64,
		longest_running_seconds = oldest.map(|age| age + waited.as_secs_f64()),
		"authority change refused: running work did not finish within the limit"
	);
	Error::PolicyBusy {
		retry_after: seconds_up(length),
	}
}

fn flag(tenant: &str) -> SimpleExpr {
	SimpleExpr::FunctionCall(
		"hashtextextended".into_iden(),
		vec![
			Expr::value(tenant.to_owned()).into(),
			Expr::value(FLAG_SEED).into(),
		],
	)
}

async fn set_local(tx: &mut dyn TransactionExecutor, name: &str, value: String) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"set_config".into_iden(),
			vec![
				Expr::value(name.to_owned()).into(),
				Expr::value(value).into(),
				Expr::value(true).into(),
			],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// Whether this transaction has raised the flag, and which database it is in.
async fn state(tx: &mut dyn TransactionExecutor) -> Result<(bool, String)> {
	let sql = Query::select()
		.expr_as(
			SimpleExpr::FunctionCall(
				"current_setting".into_iden(),
				vec![Expr::value(RAISED).into(), Expr::value(true).into()],
			),
			Alias::new("raised"),
		)
		.expr_as(
			Expr::cust("current_database()::text"),
			Alias::new("database"),
		)
		.to_string(PostgresQueryBuilder);
	let (raised, database): (Option<String>, String) = crate::database::native::query_as(&sql)
		.columns(&["raised", "database"])
		.fetch_one(tx)
		.await?;
	// A local setting leaves an empty placeholder on the session once it ends.
	Ok((raised.is_some_and(|value| !value.is_empty()), database))
}

fn remaining(started: Instant, limit: Duration) -> Duration {
	// Zero would turn the limit off.
	limit
		.saturating_sub(started.elapsed())
		.max(Duration::from_millis(1))
}

async fn limit_locks(tx: &mut dyn TransactionExecutor, limit: Duration) -> Result<()> {
	set_local(tx, "lock_timeout", format!("{}ms", limit.as_millis())).await
}

/// An Authority Change waiting for its locks. Between [`raise`] and
/// [`Self::settle`], a lock of its transaction waits only for what is left of
/// the limit.
pub struct Waiting {
	/// `None` when an earlier wait of this transaction already raised the flag.
	first: Option<First>,
}

struct First {
	tenant: String,
	database: String,
	started: Instant,
	oldest: Option<f64>,
}

/// Starts waiting as an Authority Change of `tenant`: raises the Tenant's flag
/// unless this transaction already has. Answers [`Error::PolicyBusy`] when the
/// Tenant is pausing after a refusal or the flag takes longer than the limit.
pub async fn raise(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Waiting> {
	let (raised, database) = state(&mut *tx).await?;
	if raised {
		return Ok(Waiting { first: None });
	}
	if let Some(retry_after) = paused(&database, tenant) {
		return Err(Error::PolicyBusy { retry_after });
	}
	let limit = limit();
	let started = Instant::now();
	limit_locks(&mut *tx, limit).await?;
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![flag(tenant)],
		))
		.build(PostgresQueryBuilder);
	if let Err(error) = tx.execute(&sql, convert_values(values)).await {
		let error = Error::from(error);
		return Err(if error.has_database_code("55P03") {
			refuse(&database, tenant, started.elapsed(), None)
		} else {
			error
		});
	}
	set_local(&mut *tx, RAISED, "1".to_owned()).await?;
	let oldest = oldest_holder(&mut *tx).await;
	limit_locks(&mut *tx, remaining(started, limit)).await?;
	Ok(Waiting {
		first: Some(First {
			tenant: tenant.to_owned(),
			database,
			started,
			oldest,
		}),
	})
}

impl Waiting {
	/// Ends the wait with what its lock answered: a timeout is the refusal of
	/// the change. Once held, later locks of the transaction have no limit, since
	/// a general limit would turn a legitimate wait into errors.
	pub async fn settle<T>(self, tx: &mut dyn TransactionExecutor, held: Result<T>) -> Result<T> {
		let Some(first) = self.first else {
			return held;
		};
		match held {
			Err(error) if error.has_database_code("55P03") => Err(refuse(
				&first.database,
				&first.tenant,
				first.started.elapsed(),
				first.oldest,
			)),
			Err(error) => Err(error),
			Ok(value) => {
				set_local(tx, "lock_timeout", "0".to_owned()).await?;
				metrics::histogram!("aidash_authority_change_wait_seconds")
					.record(first.started.elapsed().as_secs_f64());
				Ok(value)
			}
		}
	}
}

/// How long the oldest transaction holding a lock on the policy table has run,
/// for the warning at a refusal. A running step's transaction holds one, so it
/// is the likeliest reason a change waits, though the lock does not say which
/// Tenant's row it is on. It must be read now: a refused transaction can run
/// nothing more.
async fn oldest_holder(tx: &mut dyn TransactionExecutor) -> Option<f64> {
	let sql = Query::select()
		.expr_as(
			Expr::cust(
				"(SELECT EXTRACT(EPOCH FROM clock_timestamp() - min(a.xact_start))::float8 \
				 FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
				 WHERE l.pid <> pg_backend_pid() AND l.granted AND l.mode = 'RowShareLock' \
				 AND l.relation = 'authorization_bundles'::regclass)",
			),
			Alias::new("age"),
		)
		.to_string(PostgresQueryBuilder);
	tx.fetch_one(&sql, vec![])
		.await
		.ok()
		.and_then(|row| row.get::<f64>("age").ok())
}

/// Whether no Authority Change of `tenant` is waiting or holding its locks.
/// A failed test lets the work through, to meet the failure itself.
#[allow(clippy::disallowed_methods)] // Reads no Tenant row; a scope would slow every recheck.
pub async fn is_clear(pool: &Pool, tenant: &str) -> bool {
	let (sql, values) = Query::select()
		.expr_as(
			SimpleExpr::FunctionCall(
				"pg_try_advisory_xact_lock_shared".into_iden(),
				vec![flag(tenant)],
			),
			Alias::new("clear"),
		)
		.build(PostgresQueryBuilder);
	match pool
		.connection()
		.fetch_one(&sql, convert_values(values))
		.await
	{
		Ok(row) => row.get::<bool>("clear").unwrap_or(true),
		Err(error) => {
			tracing::debug!(%error, "authority change flag test failed");
			true
		}
	}
}

/// Waits, holding no connection, until no Authority Change of `tenant` is
/// waiting or holding its locks. Only new work calls this, and only with no
/// hold of its own on the Tenant's policy: a hold would make the change wait
/// for this wait until its limit.
pub async fn enter(pool: &Pool, tenant: &str) {
	if is_clear(pool, tenant).await {
		return;
	}
	let started = Instant::now();
	loop {
		tokio::time::sleep(RECHECK).await;
		if is_clear(pool, tenant).await {
			break;
		}
	}
	metrics::histogram!("aidash_authority_change_entrance_wait_seconds")
		.record(started.elapsed().as_secs_f64());
}

/// [`enter`] for work that holds a visibility lease, which it gives up while it
/// waits so that a change's wait holds no database connection of its own.
pub(crate) async fn enter_releasing(
	store: &crate::store::Store,
	tenant: &str,
	visibility: &mut crate::transactions::gate::ReadLease,
) -> Result<()> {
	if is_clear(&store.pool, tenant).await {
		return Ok(());
	}
	visibility.suspend().await?;
	enter(&store.pool, tenant).await;
	visibility.resume(store).await
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case(30, true)]
	#[case(21, true)]
	#[case(20, false)]
	#[case(5, false)]
	fn the_default_limit_must_fit_twice_in_the_request_limit(
		#[case] request_seconds: u64,
		#[case] accepted: bool,
	) {
		assert_eq!(
			validate(Duration::from_secs(request_seconds)).is_ok(),
			accepted
		);
	}

	#[rstest]
	#[case(Duration::from_secs(30), 30)]
	#[case(Duration::from_millis(29_001), 30)]
	#[case(Duration::from_millis(1), 1)]
	fn retry_after_rounds_up(#[case] left: Duration, #[case] seconds: u64) {
		assert_eq!(seconds_up(left), seconds);
	}

	#[test]
	fn a_pause_answers_only_for_its_own_tenant_and_database() {
		refuse("database", "paused-tenant", Duration::ZERO, None);
		assert!(paused("database", "paused-tenant").is_some());
		assert!(paused("database", "other-tenant").is_none());
		assert!(paused("other-database", "paused-tenant").is_none());
	}
}

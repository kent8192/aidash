//! Time limits on PostgreSQL statements, set per connection (ADR 0113).
//!
//! The server stops a statement at `statement_timeout`. Reaching the deadline
//! here, later by [`MARGIN`], means the server did not answer at all: SQLx sets
//! no TCP keepalive, so a reply from a server that vanished would be awaited for
//! ever. The deadline is read from the pool's own connect options, so derived
//! pools inherit it and a pool without a limit has no deadline either.
use crate::{Error, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reinhardt::core::exception::{DatabaseError, DatabaseErrorKind};
use reinhardt::db::backends::error::{PgvectorOperationKind, Result as DriverResult};
use reinhardt::db::backends::types::{RowLockCapabilities, RowStream};
use reinhardt::db::backends::{
	DatabaseBackend, DatabaseType, IsolationLevel, PostgresBackend, QueryResult, QueryValue, Row,
	TransactionExecutor,
};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr, Values,
};
use sqlx::postgres::PgConnectOptions;
use std::{future::Future, sync::Arc, time::Duration};

const SETTING: &str = "statement_timeout";
const LIMIT_ENV: &str = "AIDASH_DATABASE_STATEMENT_TIMEOUT_SECONDS";
const DEFAULT_LIMIT: Duration = Duration::from_secs(300);
const MARGIN: Duration = Duration::from_secs(30);
/// `after_release` runs with no time limit of its own in SQLx.
pub(super) const RELEASE_LIMIT: Duration = Duration::from_secs(5);

/// The configured limit; `0` turns it off.
pub(super) fn statement_limit() -> Result<Option<Duration>> {
	let Some(value) = std::env::var_os(LIMIT_ENV) else {
		return Ok(Some(DEFAULT_LIMIT));
	};
	let seconds = value
		.to_str()
		.and_then(|value| value.parse::<u64>().ok())
		.ok_or_else(|| Error::Invalid(format!("{LIMIT_ENV} must be a whole number of seconds")))?;
	Ok((seconds > 0).then(|| Duration::from_secs(seconds)))
}

/// Let the server notice a client that vanished, so its locks do not outlive it
/// by the operating system's default of hours.
pub(super) fn with_server_keepalive(options: PgConnectOptions) -> PgConnectOptions {
	options.options([
		("tcp_keepalives_idle", "15"),
		("tcp_keepalives_interval", "5"),
		("tcp_keepalives_count", "3"),
		("tcp_user_timeout", "30000"),
	])
}

pub(super) fn with_statement_limit(
	options: PgConnectOptions,
	limit: Option<Duration>,
) -> PgConnectOptions {
	match limit {
		Some(limit) => options.options([(
			"statement_timeout",
			format!("{}ms", limit.as_millis()).as_str(),
		)]),
		None => options,
	}
}

fn deadline(options: &PgConnectOptions) -> Option<Duration> {
	// Postgres applies the last of repeated settings, and this code appends its own
	// after any in `DATABASE_URL`.
	let (_, value) = options.get_options()?.rsplit_once("statement_timeout=")?;
	let digits = value.split_once("ms")?.0;
	let limit = digits.parse::<u64>().ok().filter(|limit| *limit > 0)?;
	Some(Duration::from_millis(limit) + MARGIN)
}

/// The Reinhardt backend for `pool`, bounded when the pool has a statement limit.
pub(super) fn backend(pool: sqlx::PgPool) -> Arc<dyn DatabaseBackend> {
	let deadline = deadline(pool.connect_options().as_ref());
	let inner = PostgresBackend::new(pool);
	match deadline {
		Some(deadline) => Arc::new(Bounded { inner, deadline }),
		None => Arc::new(inner),
	}
}

tokio::task_local! {
	static UNLIMITED: ();
}

/// Take the limit off the rest of `tx`, for one statement that is long by design:
/// a wait behind every read lease of the atomic gate, or a delete with no row
/// limit (#744). Run the statement through [`unlimited`], then [`restore`] with
/// the value this returns.
pub async fn lift(tx: &mut dyn TransactionExecutor) -> DriverResult<String> {
	let current = SimpleExpr::FunctionCall(
		"current_setting".into_iden(),
		vec![Expr::value(SETTING.to_owned()).into()],
	);
	let (sql, values) = Query::select()
		.expr_as(current, Alias::new("previous"))
		.expr(super::scope::set_config(SETTING, "0".to_owned()))
		.build(PostgresQueryBuilder);
	tx.fetch_one(&sql, convert_values(values))
		.await?
		.get("previous")
		.map_err(reinhardt::core::exception::Error::from)
}

/// Run `statement` with no client deadline.
pub async fn unlimited<F: Future>(statement: F) -> F::Output {
	UNLIMITED.scope((), statement).await
}

pub async fn restore(tx: &mut dyn TransactionExecutor, previous: String) -> DriverResult<()> {
	let (sql, values) = Query::select()
		.expr(super::scope::set_config(SETTING, previous))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

/// Run one statement with neither limit, in `tx`.
pub async fn execute_unlimited(
	tx: &mut dyn TransactionExecutor,
	sql: &str,
	params: Vec<QueryValue>,
) -> DriverResult<QueryResult> {
	let previous = lift(&mut *tx).await?;
	let result = unlimited(tx.execute(sql, params)).await?;
	restore(tx, previous).await?;
	Ok(result)
}

fn unanswered() -> reinhardt::core::exception::Error {
	DatabaseError::new(
		DatabaseErrorKind::Timeout,
		"The database did not answer before the statement deadline",
	)
	.into()
}

async fn timed<T>(
	deadline: Duration,
	work: impl Future<Output = DriverResult<T>>,
) -> std::result::Result<DriverResult<T>, tokio::time::error::Elapsed> {
	if UNLIMITED.try_with(|_| ()).is_ok() {
		return Ok(work.await);
	}
	tokio::time::timeout(deadline, work).await
}

async fn bounded<T>(
	deadline: Duration,
	work: impl Future<Output = DriverResult<T>>,
) -> DriverResult<T> {
	timed(deadline, work)
		.await
		.unwrap_or_else(|_| Err(unanswered()))
}

fn bounded_stream<'a>(deadline: Duration, stream: RowStream<'a>) -> RowStream<'a> {
	Box::pin(futures_util::stream::unfold(
		Some(stream),
		move |state| async move {
			let mut stream = state?;
			match bounded(deadline, async { Ok(stream.next().await) }).await {
				Ok(Some(row)) => Some((row, Some(stream))),
				Ok(None) => None,
				Err(error) => Some((Err(error), None)),
			}
		},
	))
}

struct Bounded {
	inner: PostgresBackend,
	deadline: Duration,
}

impl Bounded {
	fn transaction(&self, inner: Box<dyn TransactionExecutor>) -> Box<dyn TransactionExecutor> {
		Box::new(BoundedTransaction {
			inner,
			deadline: self.deadline,
			unanswered: false,
		})
	}
}

// Every method is forwarded, including those with default implementations: the
// defaults take a slower path than the ones `PostgresBackend` overrides.
#[async_trait]
impl DatabaseBackend for Bounded {
	fn fetch_stream_generated<'a>(
		&'a self,
		built: (String, Values),
		chunk_size: usize,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<RowStream<'a>> {
		let stream = self
			.inner
			.fetch_stream_generated(built, chunk_size, context)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	fn database_type(&self) -> DatabaseType {
		self.inner.database_type()
	}
	fn supports_pgvector_error_hints(&self) -> bool {
		self.inner.supports_pgvector_error_hints()
	}
	fn supports_row_streaming(&self) -> bool {
		self.inner.supports_row_streaming()
	}
	fn placeholder(&self, index: usize) -> String {
		self.inner.placeholder(index)
	}
	fn supports_returning(&self) -> bool {
		self.inner.supports_returning()
	}
	fn supports_on_conflict(&self) -> bool {
		self.inner.supports_on_conflict()
	}
	fn supports_transactional_ddl(&self) -> bool {
		self.inner.supports_transactional_ddl()
	}
	async fn execute_generated(
		&self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		bounded(self.deadline, self.inner.execute_generated(built, context)).await
	}
	async fn fetch_one_generated(
		&self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		bounded(
			self.deadline,
			self.inner.fetch_one_generated(built, context),
		)
		.await
	}
	async fn fetch_all_generated(
		&self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		bounded(
			self.deadline,
			self.inner.fetch_all_generated(built, context),
		)
		.await
	}
	async fn fetch_optional_generated(
		&self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Option<Row>> {
		bounded(
			self.deadline,
			self.inner.fetch_optional_generated(built, context),
		)
		.await
	}
	async fn execute(&self, sql: &str, params: Vec<QueryValue>) -> DriverResult<QueryResult> {
		bounded(self.deadline, self.inner.execute(sql, params)).await
	}
	async fn execute_with_context(
		&self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		bounded(
			self.deadline,
			self.inner.execute_with_context(sql, params, context),
		)
		.await
	}
	async fn fetch_one(&self, sql: &str, params: Vec<QueryValue>) -> DriverResult<Row> {
		bounded(self.deadline, self.inner.fetch_one(sql, params)).await
	}
	async fn fetch_one_with_context(
		&self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		bounded(
			self.deadline,
			self.inner.fetch_one_with_context(sql, params, context),
		)
		.await
	}
	async fn fetch_all(&self, sql: &str, params: Vec<QueryValue>) -> DriverResult<Vec<Row>> {
		bounded(self.deadline, self.inner.fetch_all(sql, params)).await
	}
	async fn fetch_all_with_context(
		&self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		bounded(
			self.deadline,
			self.inner.fetch_all_with_context(sql, params, context),
		)
		.await
	}
	fn fetch_stream<'a>(
		&'a self,
		sql: String,
		params: Vec<QueryValue>,
		chunk_size: usize,
	) -> DriverResult<RowStream<'a>> {
		let stream = self.inner.fetch_stream(sql, params, chunk_size)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	fn fetch_stream_with_context<'a>(
		&'a self,
		sql: String,
		params: Vec<QueryValue>,
		chunk_size: usize,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<RowStream<'a>> {
		let stream = self
			.inner
			.fetch_stream_with_context(sql, params, chunk_size, context)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	async fn fetch_optional(
		&self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> DriverResult<Option<Row>> {
		bounded(self.deadline, self.inner.fetch_optional(sql, params)).await
	}
	async fn fetch_optional_with_context(
		&self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Option<Row>> {
		bounded(
			self.deadline,
			self.inner.fetch_optional_with_context(sql, params, context),
		)
		.await
	}
	async fn __execute_generated(&self, sql: &str, values: Values) -> DriverResult<QueryResult> {
		bounded(self.deadline, self.inner.__execute_generated(sql, values)).await
	}
	async fn __fetch_one_generated(&self, sql: &str, values: Values) -> DriverResult<Row> {
		bounded(self.deadline, self.inner.__fetch_one_generated(sql, values)).await
	}
	async fn __fetch_all_generated(&self, sql: &str, values: Values) -> DriverResult<Vec<Row>> {
		bounded(self.deadline, self.inner.__fetch_all_generated(sql, values)).await
	}
	async fn begin(&self) -> DriverResult<Box<dyn TransactionExecutor>> {
		let inner = bounded(self.deadline, self.inner.begin()).await?;
		Ok(self.transaction(inner))
	}
	async fn begin_write(&self) -> DriverResult<Box<dyn TransactionExecutor>> {
		let inner = bounded(self.deadline, self.inner.begin_write()).await?;
		Ok(self.transaction(inner))
	}
	async fn begin_with_isolation(
		&self,
		isolation_level: IsolationLevel,
	) -> DriverResult<Box<dyn TransactionExecutor>> {
		let inner = bounded(
			self.deadline,
			self.inner.begin_with_isolation(isolation_level),
		)
		.await?;
		Ok(self.transaction(inner))
	}
	/// `DatabaseConnection::into_postgres` finds the pool through this.
	fn as_any(&self) -> &dyn std::any::Any {
		self.inner.as_any()
	}
}

struct BoundedTransaction {
	inner: Box<dyn TransactionExecutor>,
	deadline: Duration,
	/// Once the deadline passed, the connection is gone in all but name: nothing
	/// more is sent on it, and dropping it returns its pool slot.
	unanswered: bool,
}

impl BoundedTransaction {
	async fn call<T>(
		deadline: Duration,
		gone: &mut bool,
		work: impl Future<Output = DriverResult<T>>,
	) -> DriverResult<T> {
		if *gone {
			return Err(unanswered());
		}
		match timed(deadline, work).await {
			Ok(result) => result,
			Err(_) => {
				*gone = true;
				Err(unanswered())
			}
		}
	}
}

#[async_trait]
impl TransactionExecutor for BoundedTransaction {
	fn fetch_stream_generated<'a>(
		&'a mut self,
		built: (String, Values),
		chunk_size: usize,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<RowStream<'a>> {
		let stream = self
			.inner
			.fetch_stream_generated(built, chunk_size, context)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	async fn execute_generated(
		&mut self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.execute_generated(built, context),
		)
		.await
	}
	async fn fetch_one_generated(
		&mut self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_one_generated(built, context),
		)
		.await
	}
	async fn fetch_all_generated(
		&mut self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_all_generated(built, context),
		)
		.await
	}
	async fn fetch_optional_generated(
		&mut self,
		built: (String, Values),
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Option<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_optional_generated(built, context),
		)
		.await
	}
	async fn __fetch_one_generated(
		&mut self,
		sql: &str,
		values: Values,
		backend: DatabaseType,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.__fetch_one_generated(sql, values, backend, context),
		)
		.await
	}
	async fn __fetch_all_generated(
		&mut self,
		sql: &str,
		values: Values,
		backend: DatabaseType,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.__fetch_all_generated(sql, values, backend, context),
		)
		.await
	}
	async fn execute_generated_with_context(
		&mut self,
		sql: &str,
		values: Values,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.execute_generated_with_context(sql, values, context),
		)
		.await
	}
	async fn fetch_one_generated_with_context(
		&mut self,
		sql: &str,
		values: Values,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.fetch_one_generated_with_context(sql, values, context),
		)
		.await
	}
	async fn fetch_all_generated_with_context(
		&mut self,
		sql: &str,
		values: Values,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.fetch_all_generated_with_context(sql, values, context),
		)
		.await
	}
	fn backend(&self) -> DatabaseType {
		self.inner.backend()
	}
	fn is_cockroachdb(&self) -> bool {
		self.inner.is_cockroachdb()
	}
	fn row_lock_capabilities(&self) -> RowLockCapabilities {
		self.inner.row_lock_capabilities()
	}
	fn supports_pgvector_error_hints(&self) -> bool {
		self.inner.supports_pgvector_error_hints()
	}
	async fn __execute_generated(
		&mut self,
		sql: &str,
		values: Values,
		backend: DatabaseType,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner
				.__execute_generated(sql, values, backend, context),
		)
		.await
	}
	async fn execute(&mut self, sql: &str, params: Vec<QueryValue>) -> DriverResult<QueryResult> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.execute(sql, params),
		)
		.await
	}
	async fn execute_with_context(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<QueryResult> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.execute_with_context(sql, params, context),
		)
		.await
	}
	async fn fetch_one(&mut self, sql: &str, params: Vec<QueryValue>) -> DriverResult<Row> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_one(sql, params),
		)
		.await
	}
	async fn fetch_one_with_context(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Row> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_one_with_context(sql, params, context),
		)
		.await
	}
	async fn fetch_all(&mut self, sql: &str, params: Vec<QueryValue>) -> DriverResult<Vec<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_all(sql, params),
		)
		.await
	}
	async fn fetch_all_with_context(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Vec<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_all_with_context(sql, params, context),
		)
		.await
	}
	fn fetch_stream<'a>(
		&'a mut self,
		sql: String,
		params: Vec<QueryValue>,
		chunk_size: usize,
	) -> DriverResult<RowStream<'a>> {
		let stream = self.inner.fetch_stream(sql, params, chunk_size)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	fn fetch_stream_with_context<'a>(
		&'a mut self,
		sql: String,
		params: Vec<QueryValue>,
		chunk_size: usize,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<RowStream<'a>> {
		let stream = self
			.inner
			.fetch_stream_with_context(sql, params, chunk_size, context)?;
		Ok(bounded_stream(self.deadline, stream))
	}
	async fn fetch_optional(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> DriverResult<Option<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_optional(sql, params),
		)
		.await
	}
	async fn fetch_optional_with_context(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
		context: Option<PgvectorOperationKind>,
	) -> DriverResult<Option<Row>> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.fetch_optional_with_context(sql, params, context),
		)
		.await
	}
	async fn commit(self: Box<Self>) -> DriverResult<()> {
		if self.unanswered {
			return Err(unanswered());
		}
		bounded(self.deadline, self.inner.commit()).await
	}
	async fn rollback(self: Box<Self>) -> DriverResult<()> {
		// The server rolls back on its own once the connection is gone.
		if self.unanswered {
			return Ok(());
		}
		bounded(self.deadline, self.inner.rollback()).await
	}
	async fn savepoint(&mut self, name: &str) -> DriverResult<()> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.savepoint(name),
		)
		.await
	}
	async fn release_savepoint(&mut self, name: &str) -> DriverResult<()> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.release_savepoint(name),
		)
		.await
	}
	async fn rollback_to_savepoint(&mut self, name: &str) -> DriverResult<()> {
		Self::call(
			self.deadline,
			&mut self.unanswered,
			self.inner.rollback_to_savepoint(name),
		)
		.await
	}
}

#[cfg(test)]
mod tests;

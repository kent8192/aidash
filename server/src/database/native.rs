//! Typed projections executed exclusively by Reinhardt's native database executors.
//! SQL is compiled by Reinhardt Query at the owning repository. Projection columns
//! are explicit for tuples; database rows never acquire an invented field order.
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use async_trait::async_trait;
use reinhardt::db::backends::{
	DatabaseConnection, PostgresBackend, QueryValue, TransactionExecutor,
};
use reinhardt::db::orm::AtomicTransaction;
use reinhardt::db::orm::connection::{DatabaseConnectionLease, QueryRow};
use serde::de::{DeserializeOwned, IntoDeserializer};
use std::{marker::PhantomData, sync::Arc};

/// A Reinhardt connection owns persistence; the SQLx driver is confined to pool
/// configuration and infrastructure fixtures, never repository execution.
#[derive(Clone)]
pub struct Pool {
	connection: DatabaseConnection,
	driver: sqlx::PgPool,
	memory_recovery: Option<Arc<dyn aidash_application::ports::memory::MemoryRecovery>>,
	memory_recovery_bypass: bool,
	dashboard_policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
	#[cfg(debug_assertions)]
	fixture_driver: Arc<std::sync::OnceLock<sqlx::PgPool>>,
}
impl From<sqlx::PgPool> for Pool {
	fn from(driver: sqlx::PgPool) -> Self {
		Self {
			connection: DatabaseConnection::new(Arc::new(PostgresBackend::new(driver.clone()))),
			driver,
			memory_recovery: None,
			memory_recovery_bypass: false,
			dashboard_policy: None,
			#[cfg(debug_assertions)]
			fixture_driver: Default::default(),
		}
	}
}
impl Pool {
	pub(crate) fn with_dashboard_policy(
		mut self,
		policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Self {
		self.dashboard_policy = policy;
		self
	}
	pub(crate) fn dashboard_policy(
		&self,
	) -> Option<aidash_application::ports::authorization::dashboard::AccountPolicy> {
		self.dashboard_policy.clone()
	}

	pub(crate) fn with_memory_recovery(
		mut self,
		recovery: Option<Arc<dyn aidash_application::ports::memory::MemoryRecovery>>,
	) -> Self {
		self.memory_recovery = recovery;
		self.memory_recovery_bypass = false;
		self
	}
	pub(crate) fn memory_recovery(
		&self,
	) -> Option<Arc<dyn aidash_application::ports::memory::MemoryRecovery>> {
		self.memory_recovery.clone()
	}
	pub(crate) fn memory_recovery_internal(mut self) -> Self {
		self.memory_recovery = None;
		self.memory_recovery_bypass = true;
		self
	}
	pub(crate) fn require_memory_serving(&self) -> Result<()> {
		match &self.memory_recovery {
			Some(recovery) => Ok(recovery.require_serving()?),
			None if self.memory_recovery_bypass => Ok(()),
			None => Err(Error::SemanticUnavailable),
		}
	}
	pub fn connection(&self) -> DatabaseConnection {
		self.connection.clone()
	}
	pub fn driver(&self) -> &sqlx::PgPool {
		&self.driver
	}
	/// A second SQLx pool on the same database for test fixtures that seed or
	/// inspect rows of every Tenant directly. See
	/// [`super::scope::test_fixture_driver`].
	#[cfg(debug_assertions)]
	#[allow(clippy::disallowed_methods)] // The fixture pool itself is test-only.
	pub fn fixture_driver(&self) -> &sqlx::PgPool {
		self.fixture_driver.get_or_init(|| {
			super::scope::test_fixture_driver(
				self.driver.connect_options().as_ref().clone(),
				self.driver.options().get_max_connections(),
			)
		})
	}
	/// Open a transaction in `scope`. The scope is fixed for the transaction's
	/// lifetime; savepoints inherit it.
	#[allow(clippy::disallowed_methods)] // The scope is written before any other statement.
	pub async fn begin(&self, scope: &TenancyScope) -> Result<Transaction> {
		let mut executor = self.connection.begin().await?;
		super::scope::enter(executor.as_mut(), scope).await?;
		Ok(Transaction(
			Some(executor),
			self.clone(),
			Default::default(),
			0,
			Some(Box::new(scope.clone())),
		))
	}
	/// Like [`Self::begin`], at the serializable isolation level.
	#[allow(clippy::disallowed_methods)] // The scope is written before any other statement.
	pub async fn begin_serializable(&self, scope: &TenancyScope) -> Result<Transaction> {
		let mut executor = self
			.connection
			.begin_with_isolation(reinhardt::db::backends::IsolationLevel::Serializable)
			.await?;
		super::scope::enter(executor.as_mut(), scope).await?;
		Ok(Transaction(
			Some(executor),
			self.clone(),
			Default::default(),
			0,
			Some(Box::new(scope.clone())),
		))
	}
	/// Run `work` in a transaction opened in `scope`, committing when it
	/// succeeds. An error rolls the transaction back.
	pub async fn scoped<T>(
		&self,
		scope: &TenancyScope,
		work: impl for<'t> AsyncFnOnce(&'t mut Transaction) -> Result<T>,
	) -> Result<T> {
		let mut tx = self.begin(scope).await?;
		let value = work(&mut tx).await?;
		tx.commit().await?;
		Ok(value)
	}
	/// Run `work` in a Reinhardt ORM transaction opened in `scope`.
	#[allow(clippy::disallowed_methods)] // The scope is written before any other statement.
	pub async fn atomic<T>(
		&self,
		scope: &TenancyScope,
		work: impl for<'t> AsyncFnOnce(&'t mut AtomicTransaction) -> Result<T>,
	) -> Result<T> {
		let lease = DatabaseConnectionLease::register(self.connection.clone())?;
		lease
			.handle()
			.atomic(async |tx| {
				super::scope::enter(tx, scope).await?;
				work(tx).await
			})
			.await
	}
	/// Like [`Self::atomic`], acquiring write intent before the first read.
	#[allow(clippy::disallowed_methods)] // The scope is written before any other statement.
	pub async fn atomic_write<T>(
		&self,
		scope: &TenancyScope,
		work: impl for<'t> AsyncFnOnce(&'t mut AtomicTransaction) -> Result<T>,
	) -> Result<T> {
		let lease = DatabaseConnectionLease::register(self.connection.clone())?;
		lease
			.handle()
			.atomic_write(async |tx| {
				super::scope::enter(tx, scope).await?;
				work(tx).await
			})
			.await
	}
	/// An executor that runs each statement in its own transaction opened in
	/// `scope`, for a single statement that needs no surrounding transaction.
	pub fn in_scope<'a>(&'a self, scope: &'a TenancyScope) -> InScope<'a> {
		InScope { pool: self, scope }
	}
	pub async fn close(&self) {
		self.driver.close().await;
		#[cfg(debug_assertions)]
		if let Some(fixture) = self.fixture_driver.get() {
			fixture.close().await;
		}
	}
	pub fn options(&self) -> &sqlx::pool::PoolOptions<sqlx::Postgres> {
		self.driver.options()
	}
	pub fn connect_options(&self) -> Arc<sqlx::postgres::PgConnectOptions> {
		self.driver.connect_options()
	}
}

pub struct Transaction(
	Option<Box<dyn TransactionExecutor>>,
	Pool,
	std::collections::BTreeMap<uuid::Uuid, aidash_domain::memory::Unit>,
	/// Savepoint rollbacks so far: each may release locks taken after its savepoint.
	u64,
	/// The tenancy scope the transaction was opened in, boxed so the scope
	/// does not grow every value that holds a transaction.
	Option<Box<TenancyScope>>,
);
// A savepoint rollback issued on the deref'd executor bypasses the lock epoch.
impl std::ops::Deref for Transaction {
	type Target = dyn TransactionExecutor;
	fn deref(&self) -> &Self::Target {
		self.0
			.as_ref()
			.expect("transaction is lent to an authority scope")
			.as_ref()
	}
}
impl std::ops::DerefMut for Transaction {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.0
			.as_mut()
			.expect("transaction is lent to an authority scope")
			.as_mut()
	}
}
impl Transaction {
	pub(crate) fn lend(&mut self) -> Self {
		Self(
			Some(self.0.take().expect("active transaction")),
			self.1.clone(),
			std::mem::take(&mut self.2),
			self.3,
			self.4.clone(),
		)
	}
	/// The tenancy scope the transaction was opened in, if it was opened with one.
	pub(crate) fn scope(&self) -> Option<&TenancyScope> {
		self.4.as_deref()
	}
	pub(crate) fn restore(&mut self, lent: Self) {
		assert!(self.0.is_none());
		self.0 = lent.0;
		assert!(self.2.is_empty());
		self.2 = lent.2;
		self.3 = lent.3;
	}
	/// Changes whenever a savepoint rollback may have released locks held so far.
	pub(crate) fn lock_epoch(&self) -> u64 {
		self.3
	}
	// Savepoint statements are inherent so that method calls bind here before `Deref`
	// reaches the executor, whether or not the caller imports `TransactionExecutor`.
	pub async fn savepoint(&mut self, name: &str) -> reinhardt::db::backends::error::Result<()> {
		self.0
			.as_mut()
			.expect("active transaction")
			.savepoint(name)
			.await
	}
	pub async fn release_savepoint(
		&mut self,
		name: &str,
	) -> reinhardt::db::backends::error::Result<()> {
		self.0
			.as_mut()
			.expect("active transaction")
			.release_savepoint(name)
			.await
	}
	pub async fn rollback_to_savepoint(
		&mut self,
		name: &str,
	) -> reinhardt::db::backends::error::Result<()> {
		self.3 += 1;
		self.0
			.as_mut()
			.expect("active transaction")
			.rollback_to_savepoint(name)
			.await
	}

	pub(crate) fn pool(&self) -> &Pool {
		&self.1
	}
	pub async fn commit(self) -> Result<()> {
		self.prepare_memory_commit()?;
		match self.0 {
			Some(tx) => Ok(tx.commit().await?),
			None => Err(Error::Conflict(
				"transaction is lent to an authority scope".into(),
			)),
		}
	}
	pub async fn rollback(self) -> Result<()> {
		match self.0 {
			Some(tx) => Ok(tx.rollback().await?),
			None => Err(Error::Conflict(
				"transaction is lent to an authority scope".into(),
			)),
		}
	}
	pub(crate) fn observe_memory(&mut self, unit: &aidash_domain::memory::Unit) -> Result<()> {
		self.1.require_memory_serving()?;
		self.2.insert(unit.id, unit.clone());
		Ok(())
	}
	pub(crate) fn discard_memory_fences(&mut self) {
		self.2.clear();
	}
	pub(crate) fn require_memory_current(&self, unit: &aidash_domain::memory::Unit) -> Result<()> {
		self.1.require_memory_serving()?;
		if let Some(pending) = self.2.get(&unit.id) {
			if aidash_domain::memory::recovery::digest(pending)?
				!= aidash_domain::memory::recovery::digest(unit)?
			{
				return Err(crate::Error::SemanticUnavailable);
			}
		} else if let Some(recovery) = self.1.memory_recovery() {
			recovery.require_current(unit)?;
		}
		Ok(())
	}
	fn prepare_memory_commit(&self) -> Result<()> {
		if !self.2.is_empty()
			&& let Some(recovery) = self.1.memory_recovery()
		{
			recovery.observe_many(&self.2.values().cloned().collect::<Vec<_>>())?;
		}
		Ok(())
	}
	pub fn into_executor(self) -> Result<Box<dyn TransactionExecutor>> {
		self.prepare_memory_commit()?;
		Ok(self.0.expect("transaction is lent to an authority scope"))
	}
}
#[cfg(test)]
impl Transaction {
	/// A connectionless transaction shell for adapter state tests.
	pub(crate) fn detached(pool: Pool) -> Self {
		Self(None, pool, Default::default(), 0, None)
	}
}
impl AsMut<dyn TransactionExecutor> for Transaction {
	fn as_mut(&mut self) -> &mut (dyn TransactionExecutor + 'static) {
		self.0
			.as_mut()
			.expect("transaction is lent to an authority scope")
			.as_mut()
	}
}

pub struct Row(reinhardt::db::backends::Row);
impl Row {
	pub fn try_get<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
		let native = self
			.0
			.data
			.get(name)
			.ok_or_else(|| Error::Invalid(format!("missing projected column {name}")))?;
		let sql_null = matches!(native, QueryValue::Null | QueryValue::Json(None));
		let json = QueryRow::from_backend_row(self.0.clone()).data;
		let mut json = json;
		if let QueryValue::Bytes(bytes) = native {
			json[name] = serde_json::to_value(bytes)?;
		}
		if let QueryValue::NaiveTimestamp(timestamp) = native {
			json[name] = serde_json::Value::String(timestamp.to_string());
		}
		let value = json
			.get(name)
			.ok_or_else(|| Error::Invalid(format!("unsupported projected column {name}")))?;
		T::deserialize(Cell {
			value: value.clone(),
			sql_null,
		})
		.map_err(Error::from)
	}
	fn only_column(&self) -> Result<&str> {
		if self.0.data.len() != 1 {
			return Err(Error::Invalid(
				"scalar projection requires one column".into(),
			));
		}
		Ok(self.0.data.keys().next().expect("one projected column"))
	}
}
struct Cell {
	value: serde_json::Value,
	sql_null: bool,
}
impl<'de> serde::Deserializer<'de> for Cell {
	type Error = serde_json::Error;
	fn deserialize_any<V: serde::de::Visitor<'de>>(
		self,
		visitor: V,
	) -> std::result::Result<V::Value, Self::Error> {
		self.value.into_deserializer().deserialize_any(visitor)
	}
	fn deserialize_option<V: serde::de::Visitor<'de>>(
		self,
		visitor: V,
	) -> std::result::Result<V::Value, Self::Error> {
		if self.sql_null {
			visitor.visit_none()
		} else {
			visitor.visit_some(self.value.into_deserializer())
		}
	}
	fn deserialize_enum<V: serde::de::Visitor<'de>>(
		self,
		name: &'static str,
		variants: &'static [&'static str],
		visitor: V,
	) -> std::result::Result<V::Value, Self::Error> {
		self.value
			.into_deserializer()
			.deserialize_enum(name, variants, visitor)
	}
	fn deserialize_newtype_struct<V: serde::de::Visitor<'de>>(
		self,
		_: &'static str,
		visitor: V,
	) -> std::result::Result<V::Value, Self::Error> {
		visitor.visit_newtype_struct(self)
	}
	serde::forward_to_deserialize_any! { bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any }
}

pub trait Parameter {
	fn value(self) -> QueryValue;
}
macro_rules! parameter {
	($($ty:ty),*) => { $(impl Parameter for $ty { fn value(self) -> QueryValue { self.into() } })* };
}
parameter!(
	String,
	i64,
	i32,
	f64,
	bool,
	uuid::Uuid,
	chrono::DateTime<chrono::Utc>,
	chrono::NaiveDateTime
);
impl<T: Parameter + Clone> Parameter for &T {
	fn value(self) -> QueryValue {
		self.clone().value()
	}
}
impl Parameter for &str {
	fn value(self) -> QueryValue {
		self.into()
	}
}
impl<T: Parameter> Parameter for Option<T> {
	fn value(self) -> QueryValue {
		self.map_or(QueryValue::Null, Parameter::value)
	}
}
impl Parameter for serde_json::Value {
	fn value(self) -> QueryValue {
		QueryValue::Json(Some(Box::new(self)))
	}
}
impl Parameter for Vec<String> {
	fn value(self) -> QueryValue {
		QueryValue::StringArray(self)
	}
}
impl Parameter for Vec<uuid::Uuid> {
	fn value(self) -> QueryValue {
		QueryValue::UuidArray(self)
	}
}
impl Parameter for Vec<u8> {
	fn value(self) -> QueryValue {
		QueryValue::Bytes(self)
	}
}
impl Parameter for &[u8] {
	fn value(self) -> QueryValue {
		QueryValue::Bytes(self.to_vec())
	}
}
impl Parameter for &[String] {
	fn value(self) -> QueryValue {
		QueryValue::StringArray(self.to_vec())
	}
}
impl Parameter for &[uuid::Uuid] {
	fn value(self) -> QueryValue {
		QueryValue::UuidArray(self.to_vec())
	}
}

#[async_trait]
pub trait Executor: Send {
	async fn rows(self, sql: &str, params: Vec<QueryValue>) -> Result<Vec<Row>>;
	async fn optional(self, sql: &str, params: Vec<QueryValue>) -> Result<Option<Row>>;
	async fn execute(self, sql: &str, params: Vec<QueryValue>) -> Result<Execution>;
}
#[async_trait]
impl<T: TransactionExecutor + ?Sized> Executor for &mut T {
	async fn rows(self, sql: &str, params: Vec<QueryValue>) -> Result<Vec<Row>> {
		Ok(TransactionExecutor::fetch_all(self, sql, params)
			.await?
			.into_iter()
			.map(Row)
			.collect())
	}
	async fn optional(self, sql: &str, params: Vec<QueryValue>) -> Result<Option<Row>> {
		Ok(TransactionExecutor::fetch_optional(self, sql, params)
			.await?
			.map(Row))
	}
	async fn execute(self, sql: &str, params: Vec<QueryValue>) -> Result<Execution> {
		Ok(Execution(
			TransactionExecutor::execute(self, sql, params)
				.await?
				.rows_affected,
		))
	}
}

/// One statement in its own transaction opened in a tenancy scope.
#[derive(Clone, Copy)]
pub struct InScope<'a> {
	pool: &'a Pool,
	scope: &'a TenancyScope,
}
#[async_trait]
impl Executor for InScope<'_> {
	async fn rows(self, sql: &str, params: Vec<QueryValue>) -> Result<Vec<Row>> {
		let mut tx = self.pool.begin(self.scope).await?;
		let rows = Executor::rows(&mut tx, sql, params).await?;
		tx.commit().await?;
		Ok(rows)
	}
	async fn optional(self, sql: &str, params: Vec<QueryValue>) -> Result<Option<Row>> {
		let mut tx = self.pool.begin(self.scope).await?;
		let row = Executor::optional(&mut tx, sql, params).await?;
		tx.commit().await?;
		Ok(row)
	}
	async fn execute(self, sql: &str, params: Vec<QueryValue>) -> Result<Execution> {
		let mut tx = self.pool.begin(self.scope).await?;
		let execution = Executor::execute(&mut tx, sql, params).await?;
		tx.commit().await?;
		Ok(execution)
	}
}

pub struct Execution(u64);
impl Execution {
	pub fn rows_affected(&self) -> u64 {
		self.0
	}
}
pub trait Decode: Sized {
	fn decode(row: &Row, columns: &[&str]) -> Result<Self>;
}
impl Decode for Row {
	fn decode(row: &Row, _: &[&str]) -> Result<Self> {
		Ok(Row(row.0.clone()))
	}
}
macro_rules! tuple {
	($($ty:ident:$index:tt),+) => {
		impl<$($ty: DeserializeOwned),+> Decode for ($($ty,)+) {
			fn decode(row: &Row, columns: &[&str]) -> Result<Self> {
				if columns.len() != [$(stringify!($ty)),+].len() {
					return Err(Error::Invalid("tuple projection requires explicit columns".into()));
				}
				Ok(($(row.try_get(columns.get($index).ok_or_else(|| Error::Invalid("tuple projection requires explicit columns".into()))?)?,)+))
			}
		}
	};
}
tuple!(A:0);
tuple!(A:0,B:1);
tuple!(A:0,B:1,C:2);
tuple!(A:0,B:1,C:2,D:3);
tuple!(A:0,B:1,C:2,D:3,E:4);
tuple!(A:0,B:1,C:2,D:3,E:4,F:5);
tuple!(A:0,B:1,C:2,D:3,E:4,F:5,G:6);

pub struct Query<'q, T> {
	sql: &'q str,
	params: Vec<QueryValue>,
	columns: Vec<&'static str>,
	_type: PhantomData<T>,
}
pub fn query_as<T: Decode>(sql: &str) -> Query<'_, T> {
	Query {
		sql,
		params: Vec::new(),
		columns: Vec::new(),
		_type: PhantomData,
	}
}
pub fn query(sql: &str) -> Query<'_, Row> {
	query_as(sql)
}
pub struct Scalar<T>(T);
impl<T: DeserializeOwned> Decode for Scalar<T> {
	fn decode(row: &Row, _: &[&str]) -> Result<Self> {
		row.try_get(row.only_column()?).map(Self)
	}
}
pub fn query_scalar<T: DeserializeOwned>(sql: &str) -> Query<'_, Scalar<T>> {
	query_as(sql)
}
impl<'q, T> Query<'q, T> {
	pub fn bind(mut self, value: impl Parameter) -> Self {
		self.params.push(value.value());
		self
	}
	pub fn columns(mut self, columns: &[&'static str]) -> Self {
		self.columns = columns.to_vec();
		self
	}
	pub async fn execute(self, executor: impl Executor) -> Result<Execution> {
		Executor::execute(executor, self.sql, self.params).await
	}
}
impl<T: Decode> Query<'_, T> {
	pub async fn fetch_all(self, executor: impl Executor) -> Result<Vec<T>> {
		executor
			.rows(self.sql, self.params)
			.await?
			.into_iter()
			.map(|row| T::decode(&row, &self.columns))
			.collect()
	}
	pub async fn fetch_one(self, executor: impl Executor) -> Result<T> {
		self.fetch_optional(executor)
			.await?
			.ok_or_else(|| Error::NotFound("database row".into()))
	}
	pub async fn fetch_optional(self, executor: impl Executor) -> Result<Option<T>> {
		executor
			.optional(self.sql, self.params)
			.await?
			.as_ref()
			.map(|row| T::decode(row, &self.columns))
			.transpose()
	}
}
// Scalar requests expose the projected value rather than a transport wrapper.
impl<T: DeserializeOwned> Query<'_, Scalar<T>> {
	pub async fn scalar_all(self, executor: impl Executor) -> Result<Vec<T>> {
		Ok(self
			.fetch_all(executor)
			.await?
			.into_iter()
			.map(|v| v.0)
			.collect())
	}
	pub async fn scalar_one(self, executor: impl Executor) -> Result<T> {
		Ok(self.fetch_one(executor).await?.0)
	}
	pub async fn scalar_optional(self, executor: impl Executor) -> Result<Option<T>> {
		Ok(self.fetch_optional(executor).await?.map(|v| v.0))
	}
}

#[macro_export]
macro_rules! native_record {
	($name:ident { $($field:ident),* $(,)? }) => {
		impl $crate::database::native::Decode for $name {
			fn decode(row: &$crate::database::native::Row, _: &[&str]) -> $crate::Result<Self> {
				Ok(Self { $($field: row.try_get(stringify!($field))?,)* })
			}
		}
	};
}

#[async_trait]
impl TransactionExecutor for Transaction {
	async fn execute(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> reinhardt::db::backends::error::Result<reinhardt::db::backends::QueryResult> {
		self.0
			.as_mut()
			.expect("active transaction")
			.execute(sql, params)
			.await
	}
	async fn fetch_one(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> reinhardt::db::backends::error::Result<reinhardt::db::backends::Row> {
		self.0
			.as_mut()
			.expect("active transaction")
			.fetch_one(sql, params)
			.await
	}
	async fn fetch_all(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> reinhardt::db::backends::error::Result<Vec<reinhardt::db::backends::Row>> {
		self.0
			.as_mut()
			.expect("active transaction")
			.fetch_all(sql, params)
			.await
	}
	async fn fetch_optional(
		&mut self,
		sql: &str,
		params: Vec<QueryValue>,
	) -> reinhardt::db::backends::error::Result<Option<reinhardt::db::backends::Row>> {
		self.0
			.as_mut()
			.expect("active transaction")
			.fetch_optional(sql, params)
			.await
	}
	async fn commit(self: Box<Self>) -> reinhardt::db::backends::error::Result<()> {
		self.0.expect("active transaction").commit().await
	}
	async fn rollback(self: Box<Self>) -> reinhardt::db::backends::error::Result<()> {
		self.0.expect("active transaction").rollback().await
	}
	// `Transaction::…` paths resolve to the inherent methods above, not to these.
	async fn savepoint(&mut self, name: &str) -> reinhardt::db::backends::error::Result<()> {
		Transaction::savepoint(self, name).await
	}
	async fn release_savepoint(
		&mut self,
		name: &str,
	) -> reinhardt::db::backends::error::Result<()> {
		Transaction::release_savepoint(self, name).await
	}
	async fn rollback_to_savepoint(
		&mut self,
		name: &str,
	) -> reinhardt::db::backends::error::Result<()> {
		Transaction::rollback_to_savepoint(self, name).await
	}
}

#[cfg(test)]
pub(crate) mod probe;
#[cfg(test)]
mod tests;

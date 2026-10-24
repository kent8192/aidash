//! A connectionless executor that accepts savepoint statements and refuses queries.
use super::{Pool, Transaction};
use async_trait::async_trait;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{QueryResult, QueryValue, Row, TransactionExecutor, error::Result};
use std::sync::{Arc, Mutex};

/// Logs the savepoint statements that reach the wire, in order.
#[derive(Clone, Default)]
pub(crate) struct SavepointProbe(Arc<Mutex<Vec<&'static str>>>);

impl SavepointProbe {
	/// A transaction whose statements reach this probe instead of a connection.
	pub(crate) fn transaction(&self, pool: Pool) -> Transaction {
		Transaction(
			Some(Box::new(self.clone())),
			pool,
			Default::default(),
			0,
			None,
		)
	}
	pub(crate) fn statements(&self) -> Vec<&'static str> {
		self.0.lock().unwrap().clone()
	}
	fn log(&self, statement: &'static str) -> Result<()> {
		self.0.lock().unwrap().push(statement);
		Ok(())
	}
	fn refused<T>() -> Result<T> {
		Err(FrameworkError::Validation(
			"savepoint probe executes no queries".into(),
		))
	}
}

#[async_trait]
impl TransactionExecutor for SavepointProbe {
	async fn execute(&mut self, _: &str, _: Vec<QueryValue>) -> Result<QueryResult> {
		Self::refused()
	}
	async fn fetch_one(&mut self, _: &str, _: Vec<QueryValue>) -> Result<Row> {
		Self::refused()
	}
	async fn fetch_all(&mut self, _: &str, _: Vec<QueryValue>) -> Result<Vec<Row>> {
		Self::refused()
	}
	async fn fetch_optional(&mut self, _: &str, _: Vec<QueryValue>) -> Result<Option<Row>> {
		Self::refused()
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(())
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		Ok(())
	}
	async fn savepoint(&mut self, _: &str) -> Result<()> {
		self.log("SAVEPOINT")
	}
	async fn release_savepoint(&mut self, _: &str) -> Result<()> {
		self.log("RELEASE SAVEPOINT")
	}
	async fn rollback_to_savepoint(&mut self, _: &str) -> Result<()> {
		self.log("ROLLBACK TO SAVEPOINT")
	}
}

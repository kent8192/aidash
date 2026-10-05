//! Native query adapters keep memory writes and read checks on the inherited lease.
use super::mutations::Entries;
use crate::{Result as NativeResult, apps::knowledge::services::core as native};
use aidash_application::{
	Result,
	ports::semantic::{
		memory::{SemanticMemoryReadSession, SemanticMemoryWriteSession},
		mutations::SemanticEntriesSession,
	},
};
use aidash_domain::{
	Run,
	semantic::{
		Source,
		mutations::{Entry, History, Index},
	},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Memory<'a, 'scope> {
	pub entries: Entries<'a, 'scope>,
}

pub(crate) struct MemoryWriter<'a, 'scope> {
	pub inner: Memory<'a, 'scope>,
	pub store: crate::store::Store,
}

#[async_trait]
impl SemanticEntriesSession for MemoryWriter<'_, '_> {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		self.inner.entries.workspace(workspace, action).await
	}
	async fn index(&mut self, workspace: Uuid, exclusive: bool) -> Result<Index> {
		self.inner.entries.index(workspace, exclusive).await
	}
	async fn by_key(&mut self, workspace: Uuid, key: &str) -> Result<Option<Entry>> {
		self.inner.entries.by_key(workspace, key).await
	}
	fn saved(&mut self) -> Result<Value> {
		self.inner.entries.saved()
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.inner.entries.permits(entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.inner.entries.source(workspace, source).await
	}
	async fn count(&mut self, workspace: Uuid) -> Result<i64> {
		self.inner.entries.count(workspace).await
	}
	async fn put(&mut self, entry: Entry, authority: Value) -> Result<Entry> {
		self.inner.entries.put(entry, authority).await
	}
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()> {
		self.inner.entries.schedule(entry, collection).await
	}
	async fn history(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()> {
		self.inner
			.entries
			.history(workspace, id, revision, state, detail)
			.await
	}
	async fn list(&mut self, workspace: Uuid) -> Result<Vec<Entry>> {
		self.inner.entries.list(workspace).await
	}
	async fn lock_entry(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Entry>> {
		self.inner.entries.lock_entry(workspace, id).await
	}
	async fn reindex_replay(&mut self, id: Uuid, revision: i64) -> Result<bool> {
		self.inner.entries.reindex_replay(id, revision).await
	}
	async fn managed_memory(&mut self, id: Uuid) -> Result<Option<(String, String, String)>> {
		self.inner.entries.managed_memory(id).await
	}
	async fn require_memory_write(
		&mut self,
		workspace: Uuid,
		agent: &str,
		version: &str,
	) -> Result<()> {
		self.inner
			.entries
			.require_memory_write(workspace, agent, version)
			.await
	}
	async fn delete_memory(
		&mut self,
		workspace: Uuid,
		agent: String,
		version: String,
		home: String,
	) -> Result<()> {
		self.inner
			.entries
			.delete_memory(workspace, agent, version, home)
			.await
	}
	async fn change(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		point: Uuid,
		index_revision: i64,
		delete: bool,
		authority: Value,
	) -> Result<Entry> {
		self.inner
			.entries
			.change(workspace, id, point, index_revision, delete, authority)
			.await
	}
	async fn history_page(&mut self, workspace: Uuid, cursor: i64) -> Result<Vec<History>> {
		self.inner.entries.history_page(workspace, cursor).await
	}
	async fn history_entry(&mut self, id: Uuid) -> Result<Entry> {
		self.inner.entries.history_entry(id).await
	}
}
#[async_trait]
impl SemanticMemoryReadSession for Memory<'_, '_> {
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.entries.permits(entry, action).await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.entries.source(workspace, source).await
	}

	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>> {
		let result: NativeResult<Vec<(Uuid, i64)>> = async {
			let dependencies: Vec<(Uuid, i64)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("entry_id"))))
						.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
						.from(Alias::new("semantic_run_reads"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("entry_id"))),
							Order::Asc,
						)
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("revision"))),
							Order::Asc,
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["entry_id", "revision"])
				.fetch_all(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(dependencies)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		let result: NativeResult<native::Entry> = async {
			let entry: native::Entry = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(entry)
		}
		.await;
		result.map(|entry| Some(entry.into())).map_err(Into::into)
	}
	async fn point_digest(&mut self, point: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let digest: Option<String> = {
				let query_bind_1 = point;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("content_digest"))))
						.from(Alias::new("semantic_points"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(digest)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl SemanticMemoryWriteSession for MemoryWriter<'_, '_> {
	async fn configured(&mut self, workspace: Uuid) -> Result<bool> {
		let result: NativeResult<bool> = async {
			let exists: bool = {
				let query_bind_1 = workspace;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM semantic_indexes WHERE workspace_id = ?))"
								.to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **self.inner.entries.lease.tx())
				.await?
			};
			Ok(exists)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn revision(&mut self, workspace: Uuid, key: &str) -> Result<Option<i64>> {
		let result: NativeResult<Option<i64>> = async {
			let revision: Option<i64> = {
				let query_bind_1 = workspace;
				let query_bind_2 = &key;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND key = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **self.inner.entries.lease.tx())
				.await?
			};
			Ok(revision)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn bind_memory(&mut self, entry: &Entry, run: &Run) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = entry.id;
				let query_bind_2 = run.workspace_id;
				let query_bind_3 = &run.agent_id;
				let query_bind_4 = &run.agent_version;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("semantic_agent_memory"))
						.columns([
							Alias::new("entry_id"),
							Alias::new("workspace_id"),
							Alias::new("agent_id"),
							Alias::new("agent_version"),
							Alias::new("home_node"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(Expr::cust("''"))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns([Alias::new("entry_id")])
								.do_nothing()
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **self.inner.entries.lease.tx())
				.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn persist_memory(&mut self, run: &Run, data: &Value) -> Result<()> {
		self.store
			.remember_in(self.inner.entries.lease.tx(), run, data)
			.await
			.map_err(Into::into)
	}
}

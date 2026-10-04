//! Live authority and recorded read dependencies on a native transaction.
use super::{
	AuthorizationCatalog, AuthorizationExecution, AuthorizationRunOutput, AuthorizationRunRead,
	AuthorizationRunRegistryRead, AuthorizationRunRemoteRead, AuthorizationWorkspace,
};
use crate::{Error, Result, registry::EntityRef};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl AuthorizationWorkspace {
	pub(crate) async fn tenant_in(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
	) -> Result<Option<String>> {
		let (sql, values) = Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.lock(LockType::Share)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| row.get("tenant"))
			.transpose()
			.map_err(FrameworkError::from)?)
	}

	pub(crate) async fn owner_in(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		tenant: &str,
		lock: bool,
	) -> Result<Option<String>> {
		let mut query = Query::select();
		query
			.column(Alias::new("owner_subject"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)));
		if lock {
			query.lock(LockType::Share);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| row.get("owner_subject"))
			.transpose()
			.map_err(FrameworkError::from)?)
	}
}

impl AuthorizationCatalog {
	pub(crate) async fn enabled_in(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		entry: &EntityRef,
		lock: bool,
	) -> Result<Option<bool>> {
		let mut query = Query::select();
		query
			.column(Alias::new("enabled"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("entry_id").eq(Expr::value(&entry.id)))
			.and_where(Expr::col("entry_version").eq(Expr::value(&entry.version)));
		if lock {
			query.lock(LockType::Share);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| row.get("enabled"))
			.transpose()
			.map_err(FrameworkError::from)?)
	}
}

impl AuthorizationRunRead {
	pub(crate) async fn record_sources(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		workspace: Uuid,
		sources: &[(String, Uuid)],
	) -> Result<()> {
		for chunk in sources.chunks(256) {
			let mut query = Query::insert();
			query
				.into_table(Alias::new(Self::table_name()))
				.columns(["run_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
				.on_conflict(
					OnConflict::columns(["run_id", "resource_kind", "resource_id"]).do_nothing(),
				);
			for (kind, id) in chunk {
				query.values_panic([
					IntoValue::into_value(run),
					IntoValue::into_value(workspace),
					IntoValue::into_value(kind),
					IntoValue::into_value(*id),
				]);
			}
			let (sql, values) = query.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		Ok(())
	}

	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Vec<(Uuid, String, Uuid)>> {
		let (sql, values) = Query::select()
			.columns(["workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("resource_kind"), Order::Asc)
			.order_by(Alias::new("resource_id"), Order::Asc)
			.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				Ok((
					row.get("workspace_id").map_err(FrameworkError::from)?,
					row.get("resource_kind").map_err(FrameworkError::from)?,
					row.get("resource_id").map_err(FrameworkError::from)?,
				))
			})
			.collect()
	}
}

impl AuthorizationRunOutput {
	pub(crate) async fn record(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		let valid = AuthorizationExecution::objects()
			.filter(AuthorizationExecution::field_run_id().eq(run))
			.filter(AuthorizationExecution::field_workspace_id().eq(workspace))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?;
		if valid.is_empty() {
			return Err(Error::Forbidden);
		}
		AuthorizationRunRead::record_sources(tx, run, workspace, &[(kind.into(), id)]).await?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new(Self::table_name()))
			.columns(["run_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(run),
				IntoValue::into_value(workspace),
				IntoValue::into_value(kind),
				IntoValue::into_value(id),
			])
			.on_conflict(
				OnConflict::columns(["run_id", "resource_kind", "resource_id"]).do_nothing(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		Ok(())
	}

	pub(crate) async fn producers(
		tx: &mut dyn TransactionExecutor,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Vec<Uuid>> {
		let (sql, values) = Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("resource_kind").eq(Expr::value(kind)))
			.and_where(Expr::col("resource_id").eq(Expr::value(id)))
			.order_by(Alias::new("run_id"), Order::Asc)
			.build(PostgresQueryBuilder);
		Ok(tx
			.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| row.get("run_id"))
			.collect::<std::result::Result<_, _>>()
			.map_err(FrameworkError::from)?)
	}
}

impl AuthorizationExecution {
	pub(crate) async fn source_run_for_task(
		tx: &mut dyn TransactionExecutor,
		task: Uuid,
	) -> Result<Option<Uuid>> {
		Ok(Self::objects()
			.filter(Self::field_task_id().eq(task))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(|row| row.run_id()))
	}
}

impl AuthorizationRunRegistryRead {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Vec<EntityRef>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.order_by(&["entry_id", "entry_version"])
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|row| EntityRef {
				id: row.entry_id,
				version: row.entry_version,
			})
			.collect())
	}
}

impl AuthorizationRunRemoteRead {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Vec<(String, String, String, String, Value)>> {
		let (sql, values) = Query::select()
			.columns(["node_id", "entry_id", "entry_version", "digest", "metadata"].map(Alias::new))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("node_id"), Order::Asc)
			.order_by(Alias::new("entry_id"), Order::Asc)
			.order_by(Alias::new("entry_version"), Order::Asc)
			.order_by(Alias::new("digest"), Order::Asc)
			.build(PostgresQueryBuilder);
		tx.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				let node = row.get("node_id").map_err(FrameworkError::from)?;
				let id = row.get("entry_id").map_err(FrameworkError::from)?;
				let version = row.get("entry_version").map_err(FrameworkError::from)?;
				let hash = row.get("digest").map_err(FrameworkError::from)?;
				Ok((
					node,
					id,
					version,
					hash,
					QueryRow::from_backend_row(row).data["metadata"].clone(),
				))
			})
			.collect()
	}
}

use reinhardt::query::IntoValue;

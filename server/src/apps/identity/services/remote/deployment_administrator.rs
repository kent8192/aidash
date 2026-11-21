//! Deleted-Tenant and required-Home gates shared by every reader of Tenant content.
//! A Deployment Administrator reads no Tenant content (#484); the gates here serve
//! Subjects, the stop controls and worker scopes.
use crate::database::native::Transaction;
use crate::{Error, Result};
use reinhardt::query::{Alias, Expr, ExprTrait as _, JoinType, PostgresQueryBuilder, Query};
use std::collections::BTreeSet;
use uuid::Uuid;

fn deleted_tenants() -> reinhardt::query::SelectStatement {
	Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("tenant_deletions"))
		.and_where(Expr::col("restored_at").is_null())
		.to_owned()
}

/// Deletion takes the exclusive policy lock. The caller retains this shared
/// lock through its protected read or write.
pub(crate) async fn tenant(
	connection: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<()> {
	let result = crate::apps::identity::models::AuthorizationBundle::lock_snapshot(
		connection, tenant, false,
	)
	.await;
	// A purge removes the policy, but retains the deleted Tenant's tombstone.
	if matches!(&result, Err(Error::NotFound(_)))
		&& crate::apps::identity::repositories::tenant_deletions::is_deleted(connection, tenant)
			.await?
	{
		return Err(Error::TenantDeleted);
	}
	result.map(|_| ())
}

/// Preserve the deleted-Tenant HTTP response at public administration ports.
/// Worker ports keep their existing conversion to refused authority.
pub(crate) fn application_error(error: Error) -> aidash_application::Error {
	match error {
		error @ Error::TenantDeleted => aidash_application::Error::Port(Box::new(error)),
		error => error.into(),
	}
}

/// Index configuration is allowed for required-Home Threads, but never for
/// a deleted Tenant. The same policy lock protects configuration and its effects.
pub(crate) async fn thread_tenant(connection: &mut Transaction, thread: Uuid) -> Result<()> {
	// A receiver Run may name a foreign Thread with no local Thread row.
	// Its already resolved transaction scope still identifies the owning Tenant.
	if let Some(owner) = connection.scope().and_then(|scope| scope.tenant_name()) {
		let owner = owner.as_str().to_owned();
		return tenant(&mut **connection, &owner).await;
	}
	let query = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("threads"))
		.and_where(Expr::col("id").eq(Expr::value(thread)))
		.to_string(PostgresQueryBuilder);
	let owner: String = crate::database::native::query_scalar(&query)
		.scalar_one(&mut *connection)
		.await?;
	tenant(&mut **connection, &owner).await
}

async fn deleted_threads(connection: &mut Transaction, threads: &[Uuid]) -> Result<BTreeSet<Uuid>> {
	if threads.is_empty() {
		return Ok(BTreeSet::new());
	}
	let query = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("threads"))
		.and_where(Expr::col("id").is_in(threads.iter().copied().map(Expr::value)))
		.and_where(Expr::col("tenant").in_subquery(deleted_tenants()))
		.to_string(PostgresQueryBuilder);
	let ids: Vec<Uuid> = crate::database::native::query_scalar(&query)
		.scalar_all(&mut *connection)
		.await?;
	Ok(ids.into_iter().collect())
}

pub(crate) async fn blocked(
	connection: &mut Transaction,
	threads: &[Uuid],
) -> Result<BTreeSet<Uuid>> {
	if threads.is_empty() {
		return Ok(BTreeSet::new());
	}
	let receiver = Query::select()
		.column((Alias::new("r"), Alias::new("thread_id")))
		.from_as(Alias::new("task_executions"), Alias::new("r"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_remote_admissions"),
				Alias::new("a"),
			),
			Expr::cust("a.id=r.id"),
		)
		.and_where(Expr::cust(
			"r.thread_id=ANY($1) AND a.description->'semantic'->>'mode'='required_home'",
		))
		.to_owned();
	let query = Query::select()
		.column((Alias::new("t"), Alias::new("thread_id")))
		.from_as(Alias::new("tasks"), Alias::new("t"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(
				Alias::new("authorization_remote_grants"),
				Alias::new("g"),
			),
			Expr::cust("g.task_id=t.id"),
		)
		.and_where(Expr::cust(
			"t.thread_id=ANY($1) AND g.semantic->>'mode'='required_home'",
		))
		.union(receiver)
		.to_string(PostgresQueryBuilder);
	let values: Vec<Uuid> = crate::database::native::query_scalar(&query)
		.bind(threads)
		.scalar_all(&mut *connection)
		.await?;
	let mut denied: BTreeSet<_> = values.into_iter().collect();
	denied.extend(deleted_threads(connection, threads).await?);
	Ok(denied)
}

pub(crate) async fn visible(connection: &mut Transaction, thread: Uuid) -> Result<bool> {
	Ok(blocked(connection, &[thread]).await?.is_empty())
}

pub(crate) async fn require(connection: &mut Transaction, thread: Uuid) -> Result<()> {
	thread_tenant(connection, thread).await?;
	if visible(connection, thread).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

use reinhardt::query::QueryStatementBuilder as _;

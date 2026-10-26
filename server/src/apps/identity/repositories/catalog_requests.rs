//! Catalog Approval Requests under the caller's Tenant transaction (ADR 0096).
use crate::{Result, registry::EntityRef};
use aidash_domain::{
	identity::catalog_request::{
		CatalogApprovalRequest as Request, CatalogApprovalRequestState as State,
	},
	nodes::NodeName,
};
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, IntoValue, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SelectStatement,
};

struct RequestRow {
	id: uuid::Uuid,
	tenant: String,
	node: String,
	root_id: String,
	root_version: String,
	requester: String,
	reason: String,
	closure_digest: String,
	state: String,
	decline_reason: Option<String>,
	decided_by: Option<String>,
	created_at: DateTime<Utc>,
	updated_at: DateTime<Utc>,
	revision: i64,
}
crate::native_record!(RequestRow {
	id,
	tenant,
	node,
	root_id,
	root_version,
	requester,
	reason,
	closure_digest,
	state,
	decline_reason,
	decided_by,
	created_at,
	updated_at,
	revision
});

fn domain(row: RequestRow) -> Result<Request> {
	Ok(Request {
		id: row.id,
		tenant: row.tenant,
		node: NodeName::parse(&row.node)?,
		root: EntityRef {
			id: row.root_id,
			version: row.root_version,
		},
		requester: row.requester,
		reason: row.reason,
		closure_digest: row.closure_digest,
		state: State::parse(&row.state)?,
		decline_reason: row.decline_reason,
		decided_by: row.decided_by,
		created_at: row.created_at,
		updated_at: row.updated_at,
		revision: row.revision,
	})
}

/// Serializes create for one Tenant so the unique open request cannot race.
pub(crate) async fn lock(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
) -> Result<()> {
	let query = Query::select()
		.expr(Expr::cust(
			"pg_advisory_xact_lock(hashtextextended('catalog-approval-requests:' || $1, 0))",
		))
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&query)
		.bind(tenant)
		.execute(&mut **tx)
		.await?;
	Ok(())
}

fn select_request() -> SelectStatement {
	Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new("catalog_approval_requests"))
		.to_owned()
}

pub(crate) async fn by_id(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	id: uuid::Uuid,
) -> Result<Option<Request>> {
	let query = select_request()
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let row: Option<RequestRow> = crate::database::native::query_as(&query)
		.fetch_optional(&mut **tx)
		.await?;
	row.map(domain).transpose()
}

pub(crate) async fn open_for(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	requester: &str,
	root: &EntityRef,
	node: &NodeName,
) -> Result<Option<Request>> {
	let query = select_request()
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("requester")).eq(Expr::value(requester.to_owned())))
		.and_where(Expr::col(Alias::new("root_id")).eq(Expr::value(root.id.clone())))
		.and_where(Expr::col(Alias::new("root_version")).eq(Expr::value(root.version.clone())))
		.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.as_str().to_owned())))
		.and_where(Expr::col(Alias::new("state")).eq(Expr::value(State::Open.as_str())))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let row: Option<RequestRow> = crate::database::native::query_as(&query)
		.fetch_optional(&mut **tx)
		.await?;
	row.map(domain).transpose()
}

pub(crate) async fn open_by_root(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	node: &NodeName,
	root: &EntityRef,
) -> Result<Vec<Request>> {
	let query = select_request()
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.as_str().to_owned())))
		.and_where(Expr::col(Alias::new("root_id")).eq(Expr::value(root.id.clone())))
		.and_where(Expr::col(Alias::new("root_version")).eq(Expr::value(root.version.clone())))
		.and_where(Expr::col(Alias::new("state")).eq(Expr::value(State::Open.as_str())))
		.order_by(Alias::new("created_at"), Order::Asc)
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	crate::database::native::query_as(&query)
		.fetch_all(&mut **tx)
		.await?
		.into_iter()
		.map(domain)
		.collect()
}

pub(crate) async fn mine(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	requester: &str,
	node: Option<&NodeName>,
	offset: u64,
	limit: u64,
) -> Result<Vec<Request>> {
	let mut query = select_request();
	query
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("requester")).eq(Expr::value(requester.to_owned())));
	if let Some(node) = node {
		query.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.as_str().to_owned())));
	}
	let query = query
		.order_by(Alias::new("created_at"), Order::Desc)
		.order_by(Alias::new("id"), Order::Desc)
		.offset(offset)
		.limit(limit)
		.to_string(PostgresQueryBuilder);
	crate::database::native::query_as(&query)
		.fetch_all(&mut **tx)
		.await?
		.into_iter()
		.map(domain)
		.collect()
}

/// Every open request, bounded. The pending list filters after reviewing each
/// request, so pagination happens in memory there.
pub(crate) async fn open_page(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	node: Option<&NodeName>,
	limit: u64,
) -> Result<Vec<Request>> {
	let mut query = select_request();
	query
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant.to_owned())))
		.and_where(Expr::col(Alias::new("state")).eq(Expr::value(State::Open.as_str())));
	if let Some(node) = node {
		query.and_where(Expr::col(Alias::new("node")).eq(Expr::value(node.as_str().to_owned())));
	}
	let query = query
		.order_by(Alias::new("created_at"), Order::Asc)
		.order_by(Alias::new("id"), Order::Asc)
		.limit(limit)
		.to_string(PostgresQueryBuilder);
	crate::database::native::query_as(&query)
		.fetch_all(&mut **tx)
		.await?
		.into_iter()
		.map(domain)
		.collect()
}

pub(crate) async fn insert(
	tx: &mut crate::database::native::Transaction,
	request: &Request,
) -> Result<()> {
	let query = Query::insert()
		.into_table(Alias::new("catalog_approval_requests"))
		.columns(
			[
				"id",
				"tenant",
				"node",
				"root_id",
				"root_version",
				"requester",
				"reason",
				"closure_digest",
				"state",
				"decline_reason",
				"decided_by",
				"created_at",
				"updated_at",
				"revision",
			]
			.map(Alias::new),
		)
		.values_panic([
			IntoValue::into_value(request.id),
			IntoValue::into_value(request.tenant.clone()),
			IntoValue::into_value(request.node.as_str().to_owned()),
			IntoValue::into_value(request.root.id.clone()),
			IntoValue::into_value(request.root.version.clone()),
			IntoValue::into_value(request.requester.clone()),
			IntoValue::into_value(request.reason.clone()),
			IntoValue::into_value(request.closure_digest.clone()),
			IntoValue::into_value(request.state.as_str()),
			IntoValue::into_value(request.decline_reason.clone()),
			IntoValue::into_value(request.decided_by.clone()),
			IntoValue::into_value(request.created_at),
			IntoValue::into_value(request.updated_at),
			IntoValue::into_value(request.revision),
		])
		.to_string(PostgresQueryBuilder);
	crate::database::native::query(&query)
		.execute(&mut **tx)
		.await?;
	Ok(())
}

pub(crate) async fn save(
	tx: &mut crate::database::native::Transaction,
	request: &Request,
) -> Result<()> {
	let query = Query::update()
		.table(Alias::new("catalog_approval_requests"))
		.value(Alias::new("state"), request.state.as_str())
		.value(Alias::new("decline_reason"), request.decline_reason.clone())
		.value(Alias::new("decided_by"), request.decided_by.clone())
		.value(Alias::new("updated_at"), request.updated_at)
		.value(Alias::new("revision"), request.revision)
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(request.tenant.clone())))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(request.id)))
		.and_where(Expr::col(Alias::new("revision")).eq(Expr::value(request.revision - 1)))
		.to_string(PostgresQueryBuilder);
	let affected = crate::database::native::query(&query)
		.execute(&mut **tx)
		.await?
		.rows_affected();
	if affected != 1 {
		return Err(crate::Error::Conflict(
			"catalog approval request changed".into(),
		));
	}
	Ok(())
}

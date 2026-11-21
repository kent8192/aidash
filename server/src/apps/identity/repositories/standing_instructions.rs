//! Tenant Instructions and Node Instructions (#1010). Each text has a revision
//! for optimistic updates; a Tenant or Node that never stored one reads as an
//! empty text at revision 0.
use crate::{Error, Result, database::native};
use aidash_domain::standing_instructions::{StandingInstructions, validate_standing_text};
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

/// One stored text, or the empty text at revision 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stored {
	pub text: String,
	pub revision: i64,
	pub updated_at: Option<DateTime<Utc>>,
	pub updated_by: Option<String>,
}

/// Which text: the Tenant's, or one Node's.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Target<'a> {
	Tenant,
	Node(&'a str),
}

impl Target<'_> {
	fn table(self) -> &'static str {
		match self {
			Self::Tenant => "tenant_instructions",
			Self::Node(_) => "node_instructions",
		}
	}
}

/// Locks the row the text belongs to, the Tenant or the Node, so a write
/// serialises with the Node's deletion; an unknown Node is not found.
async fn lock_owner(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	target: Target<'_>,
	lock: LockType,
) -> Result<()> {
	let query = match target {
		Target::Tenant => Query::select()
			.column(Alias::new("name"))
			.from(Alias::new("tenants"))
			.and_where(Expr::col("name").eq(tenant))
			.lock(lock)
			.to_string(PostgresQueryBuilder),
		Target::Node(node) => Query::select()
			.column(Alias::new("name"))
			.from(Alias::new("nodes"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("name").eq(node))
			.lock(lock)
			.to_string(PostgresQueryBuilder),
	};
	if native::query(&query)
		.fetch_optional(&mut *tx)
		.await?
		.is_none()
	{
		return Err(Error::NotFound(match target {
			Target::Tenant => "tenant not found".into(),
			Target::Node(_) => "node not found".into(),
		}));
	}
	Ok(())
}

async fn read(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	target: Target<'_>,
) -> Result<Stored> {
	let mut query = Query::select();
	query
		.columns(["text", "revision", "updated_at", "updated_by"].map(Alias::new))
		.from(Alias::new(target.table()))
		.and_where(Expr::col("tenant").eq(tenant));
	if let Target::Node(node) = target {
		query.and_where(Expr::col("node").eq(node));
	}
	let row = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_optional(tx)
		.await?;
	Ok(match row {
		Some(row) => Stored {
			text: row.try_get("text")?,
			revision: row.try_get("revision")?,
			updated_at: Some(row.try_get("updated_at")?),
			updated_by: Some(row.try_get("updated_by")?),
		},
		None => Stored::default(),
	})
}

/// The current text of the Tenant or of one of its Nodes.
pub(crate) async fn get(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	target: Target<'_>,
) -> Result<Stored> {
	lock_owner(tx, tenant, target, LockType::KeyShare).await?;
	read(tx, tenant, target).await
}

/// Replaces the text when `expected` is still the current revision, and
/// returns the new revision. A stale revision is a conflict.
pub(crate) async fn set(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	target: Target<'_>,
	text: &str,
	expected: i64,
	by: &str,
) -> Result<i64> {
	validate_standing_text(text).map_err(|error| Error::Invalid(error.to_string()))?;
	lock_owner(tx, tenant, target, LockType::Update).await?;
	let current = read(tx, tenant, target).await?.revision;
	if current != expected || current == i64::MAX {
		return Err(Error::Conflict("instructions revision changed".into()));
	}
	let revision = current + 1;
	let mut columns = vec!["tenant"];
	let mut select = Query::select();
	select.expr(Expr::value(tenant));
	if let Target::Node(node) = target {
		columns.push("node");
		select.expr(Expr::value(node));
	}
	columns.extend(["text", "revision", "updated_at", "updated_by"]);
	select
		.expr(Expr::value(text))
		.expr(Expr::value(revision))
		.expr(Expr::value(Utc::now()))
		.expr(Expr::value(by));
	let conflict = match target {
		Target::Tenant => OnConflict::column(Alias::new("tenant")),
		Target::Node(_) => OnConflict::columns(["tenant", "node"].map(Alias::new)),
	};
	native::query(
		&Query::insert()
			.into_table(Alias::new(target.table()))
			.columns(columns.into_iter().map(Alias::new))
			.from_subquery(select.to_owned())
			.on_conflict(
				conflict
					.update_columns(
						["text", "revision", "updated_at", "updated_by"].map(Alias::new),
					)
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(revision)
}

/// The texts a Run working in `node` pins when it is claimed.
pub(crate) async fn standing(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	node: &str,
) -> Result<StandingInstructions> {
	Ok(StandingInstructions {
		tenant: read(tx, tenant, Target::Tenant).await?.text,
		node: read(tx, tenant, Target::Node(node)).await?.text,
		duty: String::new(),
	})
}

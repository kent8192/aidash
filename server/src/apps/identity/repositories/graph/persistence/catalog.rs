//! Filter catalog kinds before pagination consumes the candidate scan budget.
use super::{Candidate, GraphAuthority, GraphOptions, kind_allowed};
use crate::{Error, Result};
use reinhardt::query::{
	Alias, Condition, Expr, JoinType, LockType, Order, PostgresQueryBuilder, Query,
};
use serde_json::Value;

fn query(tenant: &str, options: &GraphOptions, offset: u64) -> Option<(String, Vec<String>)> {
	let kinds: Vec<String> = ["agent", "cluster", "tool", "model", "skill"]
		.into_iter()
		.filter(|kind| kind_allowed(kind, options))
		.map(str::to_owned)
		.collect();
	if kinds.is_empty() {
		return None;
	}
	let sql = Query::select()
		.column((Alias::new("r"), Alias::new("metadata")))
		.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
			Condition::all()
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("id"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
				)
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("r"),
						Alias::new("version"),
					)))
					.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
				),
		)
		.and_where(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::value(tenant)))
		.and_where(
			Expr::col((Alias::new("c"), Alias::new("enabled")))
				.eq(reinhardt::query::Expr::value(true)),
		)
		.and_where(Expr::cust("r.metadata->>'kind'").is_in(kinds.iter().cloned()))
		.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
		.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
		.limit(super::CANDIDATE_BATCH)
		.offset(offset)
		.lock(LockType::Share)
		.lock_tables([Alias::new("c")])
		.to_string(PostgresQueryBuilder);
	Some((sql, kinds))
}

pub(super) async fn candidates(
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	offset: u64,
) -> Result<Vec<Candidate>> {
	let tenant = authority.tenant().to_owned();
	let Some((sql, _)) = query(&tenant, options, offset) else {
		return Ok(vec![]);
	};
	let documents: Vec<Value> = crate::database::native::query_scalar(&sql)
		.scalar_all(authority.connection())
		.await?;
	documents
		.into_iter()
		.map(|value| {
			serde_json::from_value(value)
				.map(Candidate::Registry)
				.map_err(Error::from)
		})
		.collect()
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

#[cfg(test)]
#[path = "../../../tests/services_peer_graph_catalog_tests.rs"]
mod tests;

//! Filter catalog kinds before pagination consumes the candidate scan budget.
use super::{Candidate, GraphAuthority, GraphOptions, kind_allowed};
use crate::{Error, Result};
use sea_orm::sea_query::{
	Alias, Condition, Expr, JoinType, LockType, Order, PostgresQueryBuilder, Query,
};
use serde_json::Value;

fn query(options: &GraphOptions, offset: u64) -> Option<(String, Vec<String>)> {
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
		.join_as(
			JoinType::InnerJoin,
			Alias::new("registry"),
			Alias::new("r"),
			Condition::all()
				.add(
					Expr::col((Alias::new("r"), Alias::new("id")))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
				)
				.add(
					Expr::col((Alias::new("r"), Alias::new("version")))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
				),
		)
		.and_where(Expr::col((Alias::new("c"), Alias::new("tenant"))).eq(Expr::cust("$1")))
		.and_where(Expr::col((Alias::new("c"), Alias::new("enabled"))).eq(true))
		.and_where(Expr::cust("r.metadata->>'kind' = ANY($2::text[])"))
		.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
		.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
		.limit(super::CANDIDATE_BATCH)
		.offset(offset)
		.lock_with_tables(LockType::Share, [Alias::new("c")])
		.to_string(PostgresQueryBuilder);
	Some((sql, kinds))
}

pub(super) async fn candidates(
	authority: &mut GraphAuthority<'_>,
	options: &GraphOptions,
	offset: u64,
) -> Result<Vec<Candidate>> {
	let Some((sql, kinds)) = query(options, offset) else {
		return Ok(vec![]);
	};
	let tenant = authority.tenant().to_owned();
	let documents: Vec<Value> = sqlx::query_scalar(&sql)
		.bind(tenant)
		.bind(kinds)
		.fetch_all(authority.connection())
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

#[cfg(test)]
mod tests {
	use super::*;

	fn options(mode: &str, kinds: &[&str]) -> GraphOptions {
		GraphOptions {
			scope_workspace: None,
			depth: 1,
			mode: mode.into(),
			kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
			relations: vec![],
			hours: 0,
			limit: 80,
			cursor: None,
			target_tenant: None,
		}
	}

	#[test]
	fn kind_filter_precedes_pagination() {
		let (sql, kinds) = query(&options("mesh", &["agent", "run"]), 4096).unwrap();
		assert_eq!(kinds, vec!["agent"]);
		let predicate = sql.find("r.metadata->>'kind' = ANY($2::text[])").unwrap();
		assert!(predicate < sql.find("LIMIT 64").unwrap());
		assert!(sql.contains("OFFSET 4096"));
	}

	#[test]
	fn perspective_filters_catalog_kinds() {
		let (_, kinds) =
			query(&options("execution", &["agent", "tool", "model", "run"]), 0).unwrap();
		assert_eq!(kinds, vec!["agent"]);
		let (_, kinds) =
			query(&options("topology", &["agent", "tool", "model"]), 0).unwrap();
		assert_eq!(kinds, vec!["agent", "tool"]);
	}

	#[test]
	fn irrelevant_kinds_skip_the_catalog_query() {
		assert!(query(&options("mesh", &["workspace", "goal", "run"]), 0).is_none());
		assert!(query(&options("execution", &["tool", "model"]), 0).is_none());
		assert!(query(&options("mesh", &[]), 0).is_none());
	}
}

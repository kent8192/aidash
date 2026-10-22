//! The order in which purge deletes a Tenant's rows (ADR 0061).
//!
//! The checked-in classification says which tables hold a Tenant's data; the
//! migrated database says which tables reference which. Purge reads the second
//! to order the first, so a table added to the classification is purged
//! without anyone keeping a second list.
use crate::{Error, Result};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// A foreign key: `child` holds rows that reference rows of `parent`, through
/// its `columns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
	pub child: String,
	pub parent: String,
	pub columns: Vec<String>,
	/// Whether deleting a parent row deletes its child rows with it.
	pub cascades: bool,
}

/// A nullable column whose foreign key purge ignores when it orders deletions,
/// because it closes a cycle or points from a table purge keeps into one it
/// deletes. Purge sets the column NULL for the Tenant's rows first. A test
/// requires each to be nullable, to be part of a foreign key and to be needed.
pub type Unlink = (&'static str, &'static str);

fn unlinked(reference: &Reference, unlinks: &[Unlink]) -> bool {
	unlinks.iter().any(|(table, column)| {
		*table == reference.child && reference.columns.iter().any(|name| name == column)
	})
}

/// Orders `tables` so that every table comes before the tables it references:
/// deleting in this order never removes a row that another row to be deleted
/// still references. A table referencing itself is no constraint, because one
/// `DELETE` removes all its rows together. References to tables outside
/// `tables` are ignored here; the classification test refuses those that would
/// block a purge. A reference through an `unlinks` column is ignored too, and so
/// is one that cascades, because the parent's deletion removes the child rows.
pub fn deletion_order(
	tables: impl IntoIterator<Item = &'static str>,
	references: &[Reference],
	unlinks: &[Unlink],
) -> Result<Vec<&'static str>> {
	let tables = tables.into_iter().collect::<BTreeSet<_>>();
	// How many tables of the set still have to be deleted before a table can go.
	let mut blockers = tables
		.iter()
		.map(|table| (*table, BTreeSet::new()))
		.collect::<BTreeMap<&str, BTreeSet<&str>>>();
	for reference in references
		.iter()
		.filter(|reference| !reference.cascades && !unlinked(reference, unlinks))
	{
		let (Some(parent), Some(child)) = (
			tables.get(reference.parent.as_str()),
			tables.get(reference.child.as_str()),
		) else {
			continue;
		};
		if parent != child {
			blockers
				.get_mut(parent)
				.expect("every table has an entry")
				.insert(*child);
		}
	}
	let mut order = Vec::with_capacity(tables.len());
	while order.len() < tables.len() {
		let ready = blockers
			.iter()
			.filter(|(_, children)| children.is_empty())
			.map(|(table, _)| *table)
			.collect::<Vec<_>>();
		if ready.is_empty() {
			return Err(Error::Invalid(format!(
				"purge cannot order tables that reference each other in a cycle: {}",
				blockers.keys().copied().collect::<Vec<_>>().join(", ")
			)));
		}
		for table in ready {
			blockers.remove(table);
			for children in blockers.values_mut() {
				children.remove(table);
			}
			order.push(*tables.get(table).expect("a ready table is in the set"));
		}
	}
	Ok(order)
}

/// Every foreign key between tables of the `public` schema, read from the
/// migrated database.
pub async fn references(tx: &mut dyn TransactionExecutor) -> Result<Vec<Reference>> {
	let (sql, values) = Query::select()
		.expr_as(
			Expr::cust("(SELECT relname::text FROM pg_class WHERE oid = pg_constraint.conrelid)"),
			Alias::new("child"),
		)
		.expr_as(
			Expr::cust("(SELECT relname::text FROM pg_class WHERE oid = pg_constraint.confrelid)"),
			Alias::new("parent"),
		)
		.expr_as(
			Expr::cust("pg_constraint.confdeltype = 'c'"),
			Alias::new("cascades"),
		)
		.expr_as(
			Expr::cust(
				"(SELECT string_agg(a.attname::text, ',' ORDER BY k.ord) \
				 FROM unnest(pg_constraint.conkey) WITH ORDINALITY k(attnum, ord) \
				 JOIN pg_attribute a ON a.attrelid = pg_constraint.conrelid AND a.attnum = k.attnum)",
			),
			Alias::new("columns"),
		)
		.from(Alias::new("pg_constraint"))
		.and_where(Expr::col("contype").eq(Expr::value("f")))
		.and_where(Expr::cust("connamespace = 'public'::regnamespace"))
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			Ok(Reference {
				child: row
					.get("child")
					.map_err(reinhardt::core::exception::Error::from)?,
				parent: row
					.get("parent")
					.map_err(reinhardt::core::exception::Error::from)?,
				cascades: row
					.get("cascades")
					.map_err(reinhardt::core::exception::Error::from)?,
				columns: row
					.get::<String>("columns")
					.map_err(reinhardt::core::exception::Error::from)?
					.split(',')
					.map(str::to_owned)
					.collect(),
			})
		})
		.collect()
}

/// Deletes every row `tenant` owns in `table`, and reports how many went.
pub async fn delete_tenant_rows(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	tenant: &str,
) -> Result<u64> {
	let (sql, values) = Query::delete()
		.from_table(Alias::new(table))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.build(PostgresQueryBuilder);
	Ok(tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected)
}

/// Clears the `unlinks` columns of the rows `tenant` owns, so the tables they
/// reference can be deleted before the tables that reference them back.
pub async fn unlink(
	tx: &mut dyn TransactionExecutor,
	unlinks: &[Unlink],
	tenant: &str,
) -> Result<()> {
	for (table, column) in unlinks {
		let (sql, values) = Query::update()
			.table(Alias::new(*table))
			.value_expr(Alias::new(*column), Expr::value(None::<Uuid>))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col(Alias::new(*column)).is_not_null())
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;

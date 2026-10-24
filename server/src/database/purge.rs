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
	/// Referenced columns in matching ordinal order.
	pub parent_columns: Vec<String>,
	/// SET NULL/SET DEFAULT updates a surviving child and changes its CTID.
	pub updates_child: bool,
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
	// Read ordinal arrays as text rather than embedding unnest/WITH ORDINALITY
	// SQL. All three catalog reads use supported Query expressions. Restricting
	// the class list to public also excludes extension-internal foreign keys.
	let public = Query::select()
		.column(Alias::new("oid"))
		.from(Alias::new("pg_namespace"))
		.and_where(Expr::col("nspname").eq(Expr::value("public")))
		.to_owned();
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col("oid").cast_as(Alias::new("text")),
			Alias::new("oid"),
		)
		.expr_as(
			Expr::col("relname").cast_as(Alias::new("text")),
			Alias::new("relname"),
		)
		.from(Alias::new("pg_class"))
		.and_where(Expr::col("relnamespace").in_subquery(public))
		.build(PostgresQueryBuilder);
	let mut names = BTreeMap::<String, String>::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		names.insert(
			row.get("oid")
				.map_err(reinhardt::core::exception::Error::from)?,
			row.get("relname")
				.map_err(reinhardt::core::exception::Error::from)?,
		);
	}
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col("attrelid").cast_as(Alias::new("text")),
			Alias::new("relation"),
		)
		.expr_as(
			Expr::col("attnum").cast_as(Alias::new("int8")),
			Alias::new("number"),
		)
		.expr_as(
			Expr::col("attname").cast_as(Alias::new("text")),
			Alias::new("attname"),
		)
		.from(Alias::new("pg_attribute"))
		.and_where(Expr::col("attnum").gt(Expr::value(0_i32)))
		.and_where(Expr::col("attisdropped").eq(Expr::value(false)))
		.build(PostgresQueryBuilder);
	let mut attributes = BTreeMap::<(String, i64), String>::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		attributes.insert(
			(
				row.get("relation")
					.map_err(reinhardt::core::exception::Error::from)?,
				row.get("number")
					.map_err(reinhardt::core::exception::Error::from)?,
			),
			row.get("attname")
				.map_err(reinhardt::core::exception::Error::from)?,
		);
	}
	let mut query = Query::select();
	for field in ["conrelid", "confrelid", "conkey", "confkey", "confdeltype"] {
		query.expr_as(
			Expr::col(Alias::new(field)).cast_as(Alias::new("text")),
			Alias::new(field),
		);
	}
	let (sql, values) = query
		.from(Alias::new("pg_constraint"))
		.and_where(Expr::col("contype").eq(Expr::value("f")))
		.build(PostgresQueryBuilder);
	let mut references = Vec::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let child_oid: String = row
			.get("conrelid")
			.map_err(reinhardt::core::exception::Error::from)?;
		let parent_oid: String = row
			.get("confrelid")
			.map_err(reinhardt::core::exception::Error::from)?;
		let (Some(child), Some(parent)) = (names.get(&child_oid), names.get(&parent_oid)) else {
			continue;
		};
		let column_names = |field: &str, oid: &str| -> Result<Vec<String>> {
			let array: String = row
				.get(field)
				.map_err(reinhardt::core::exception::Error::from)?;
			array
				.trim_start_matches('{')
				.trim_end_matches('}')
				.split(',')
				.map(|number| {
					let number = number
						.parse::<i64>()
						.map_err(|_| Error::Invalid("invalid FK catalog ordinal".into()))?;
					attributes
						.get(&(oid.to_owned(), number))
						.cloned()
						.ok_or_else(|| Error::Invalid("missing FK catalog attribute".into()))
				})
				.collect()
		};
		let action: String = row
			.get("confdeltype")
			.map_err(reinhardt::core::exception::Error::from)?;
		references.push(Reference {
			child: child.clone(),
			parent: parent.clone(),
			columns: column_names("conkey", &child_oid)?,
			parent_columns: column_names("confkey", &parent_oid)?,
			cascades: action == "c",
			updates_child: matches!(action.as_str(), "n" | "d"),
		});
	}
	Ok(references)
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

//! Locked row snapshots and FK ownership validation. Never delete through a
//! guessed node column, and never let CASCADE cross the selected owner set.
use super::*;
use crate::database::classification::node::Ownership;
use reinhardt::query::{JoinType, SimpleExpr};
use std::collections::BTreeMap;

pub(super) async fn coverage(tx: &mut dyn TransactionExecutor) -> Result<()> {
	let (sql, values) = Query::select()
		.expr_as(
			selectors::col("pg_class", "relname").cast_as(Alias::new("text")),
			Alias::new("relname"),
		)
		.from(Alias::new("pg_class"))
		.join(
			JoinType::InnerJoin,
			Alias::new("pg_namespace"),
			selectors::col("pg_namespace", "oid").eq(selectors::col("pg_class", "relnamespace")),
		)
		.and_where(selectors::col("pg_namespace", "nspname").eq(Expr::value("public")))
		.and_where(selectors::col("pg_class", "relkind").is_in(["r", "p"]))
		.build(PostgresQueryBuilder);
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		classification::node::ownership(
			&row.get::<String>("relname")
				.map_err(reinhardt::core::exception::Error::from)?,
		)?;
	}
	for table in crate::bootstrap::build::current().tables() {
		classification::node::ownership(table.name)?;
	}
	Ok(())
}
fn tables() -> impl Iterator<Item = &'static str> {
	crate::bootstrap::build::current()
		.tables()
		.filter(|table| {
			classification::node::ownership(table.name).is_ok_and(|owner| {
				!matches!(
					owner,
					Ownership::Shared | Ownership::Recovery | Ownership::Audit | Ownership::Objects
				)
			})
		})
		.map(|table| table.name)
}

/// Check every inbound FK before unlinking files. A surviving sibling mapping
/// into a target credential, or an unexpected CASCADE, blocks the purge.
pub(super) async fn validate(tx: &mut dyn TransactionExecutor, owners: &Owners<'_>) -> Result<()> {
	coverage(tx).await?;
	let selected = tables().collect::<Vec<_>>();
	for reference in purge::references(tx).await? {
		if !selected.contains(&reference.parent.as_str()) {
			continue;
		}
		let parent = &reference.parent;
		let child = &reference.child;
		let mut query = Query::select();
		query
			.expr(Expr::value(1_i32))
			.from_as(Alias::new(child), Alias::new("node_purge_child"));
		let mut linked: SimpleExpr = Expr::value(true).into();
		for (child_col, parent_col) in reference.columns.iter().zip(&reference.parent_columns) {
			linked = linked.and(
				selectors::col("node_purge_child", child_col)
					.eq(selectors::col(parent, parent_col)),
			);
		}
		query.and_where(Expr::exists(
			Query::select()
				.expr(Expr::value(1_i32))
				.from(Alias::new(parent))
				.and_where(owners.predicate(parent)?)
				.and_where(linked)
				.to_owned(),
		));
		// NULL owner predicates must count as surviving; IS NOT TRUE also covers
		// nullable workspace/area links without weakening a cross-owner guard.
		query
			.and_where(selectors::not_true(
				owners.predicate_at(child, "node_purge_child")?,
			))
			.limit(1);
		let (sql, values) = query.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
		{
			return Err(Error::Conflict(
				if child == "memory_remote_reads" && parent == "memory_units" {
					"Node purge blocked by a surviving memory read retaining a target Unit; release requires the read owner authorized lifecycle".into()
				} else {
					format!(
						"Node purge blocked by a surviving {child} reference to {parent}; erasure requires the surviving owner authorized lifecycle"
					)
				},
			));
		}
		if reference.cascades || reference.updates_child {
			let same_tenant = reference
				.columns
				.iter()
				.zip(&reference.parent_columns)
				.any(|(a, b)| a == "tenant" && b == "tenant");
			if !same_tenant || reference.updates_child {
				let (sql, values) = Query::select()
					.expr(Expr::value(1_i32))
					.from(Alias::new(parent))
					.and_where(owners.predicate(parent)?)
					.limit(1)
					.build(PostgresQueryBuilder);
				if tx
					.fetch_optional(&sql, convert_values(values))
					.await?
					.is_some()
				{
					return Err(Error::Conflict(format!(
						"Node purge cannot prove the {child} to {parent} delete action is owner confined"
					)));
				}
			}
		}
	}
	Ok(())
}

pub(super) async fn erase(tx: &mut dyn TransactionExecutor, owners: &Owners<'_>) -> Result<()> {
	// Source deletion invalidates disposable body copies even in surviving Home
	// journals. Do this before CTID snapshots: invalidation updates journal rows.
	let (sql, values) = Query::select()
		.columns(["id", "revision"].map(Alias::new))
		.from(Alias::new("memory_units"))
		.and_where(owners.predicate("memory_units")?)
		.order_by(Alias::new("id"), Order::Asc)
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let unit: Uuid = row
			.get("id")
			.map_err(reinhardt::core::exception::Error::from)?;
		let revision: i64 = row
			.get("revision")
			.map_err(reinhardt::core::exception::Error::from)?;
		crate::apps::knowledge::repositories::remote_memory_reads::erase_receipts_in(
			tx,
			owners.tenant,
			unit,
			revision,
			true,
		)
		.await?;
	}
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("semantic_entries"))
		.and_where(owners.predicate("semantic_entries")?)
		.order_by(Alias::new("id"), Order::Asc)
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let entry: Uuid = row
			.get("id")
			.map_err(reinhardt::core::exception::Error::from)?;
		crate::apps::knowledge::repositories::remote_memory_reads::erase_entry_receipts_in(
			tx,
			owners.tenant,
			entry,
		)
		.await?;
	}
	// The verified doomed parent owns its dependency array. Clear only that
	// parent data through the existing sync trigger before CTID capture; direct
	// task_dependencies DML is forbidden. This also handles a valid parent B /
	// child A graph where B depends on A and leaf-only DELETE cannot proceed.
	let (sql, values) = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("tasks"))
		.and_where(owners.predicate("tasks")?)
		.lock(LockType::Update)
		.build(PostgresQueryBuilder);
	let tasks = tx
		.fetch_all(&sql, convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			row.get::<Uuid>("id")
				.map_err(reinhardt::core::exception::Error::from)
		})
		.collect::<std::result::Result<Vec<_>, _>>()?;
	validate(tx, owners).await?;
	if !tasks.is_empty() {
		let empty = reinhardt::query::Value::Array(
			reinhardt::query::ArrayType::Uuid,
			Some(Box::new(Vec::new())),
		);
		let (sql, values) = Query::update()
			.table(Alias::new("tasks"))
			.value_expr(Alias::new("dependencies"), Expr::value(empty))
			.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
			.and_where(selectors::ids(Expr::col("id").into(), &tasks))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	let mut snapshot = BTreeMap::<&'static str, Vec<String>>::new();
	// Lock all prospective parents before revalidating cascades. Referencing
	// inserts now wait on these rows; no selected row is updated before DELETE.
	for table in tables() {
		let (sql, values) = Query::select()
			.expr_as(
				selectors::col(table, "ctid").cast_as(Alias::new("text")),
				Alias::new("location"),
			)
			.from(Alias::new(table))
			.and_where(owners.predicate(table)?)
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		let ids = tx
			.fetch_all(&sql, convert_values(values))
			.await?
			.into_iter()
			.map(|r| {
				r.get::<String>("location")
					.map_err(reinhardt::core::exception::Error::from)
			})
			.collect::<std::result::Result<Vec<_>, _>>()?;
		snapshot.insert(table, ids);
	}
	validate(tx, owners).await?;
	let references = purge::references(tx).await?;
	for table in purge::deletion_order(snapshot.keys().copied(), &references, &[])? {
		// Statement-level task_dependencies guard only permits parent-maintained
		// cascades in Tenant scope, including a DELETE that would affect zero rows.
		if classification::node::ownership(table)? == Ownership::CascadeTasks {
			continue;
		}
		let ids = &snapshot[table];
		if ids.is_empty() {
			continue;
		}
		let (sql, values) = Query::delete()
			.from_table(Alias::new(table))
			.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
			.and_where(selectors::ctids(table, ids))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

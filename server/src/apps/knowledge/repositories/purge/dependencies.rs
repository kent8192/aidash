//! Erase withdrawn quotations, including old revisions and selected shared copies.
use super::super::{access::Lease, units};
use crate::{Error, Result, database::native};
use aidash_domain::memory::{Bounds, Unit};
use reinhardt::query::types::PgBinOper;
use reinhardt::query::{
	Alias, BinOper, ColumnRef, Condition, Expr, ExprTrait, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use std::collections::{BTreeSet, VecDeque};
use uuid::Uuid;

pub(super) async fn erase(
	lease: &mut Lease<'_>,
	root: &Unit,
	bank: Uuid,
	bounds: &Bounds,
) -> Result<BTreeSet<Uuid>> {
	let mut pending = VecDeque::from([root.id]);
	let mut seen = BTreeSet::from([root.id]);
	let mut banks = BTreeSet::from([bank]);
	while let Some(id) = pending.pop_front() {
		discover(
			lease,
			root,
			bounds,
			Condition::any().add(contains("unit", id)),
			&mut pending,
			&mut seen,
			&mut banks,
		)
		.await?;
		// Read/publication fanout is a durable journal, not an admitted unit graph.
		// Keyset pages bound each query and predicate without rejecting its total size.
		let mut cursor = None;
		loop {
			let page = references(
				lease,
				"memory_publications",
				"id",
				"source_id",
				id,
				cursor,
				bounds.max_graph_visits,
			)
			.await?;
			let Some(last) = page.last().copied() else {
				break;
			};
			cursor = Some(last);
			let mut support = Condition::any();
			for publication in page {
				support = support.add(contains("publication", publication));
			}
			discover(
				lease,
				root,
				bounds,
				support,
				&mut pending,
				&mut seen,
				&mut banks,
			)
			.await?;
		}
		let mut cursor = None;
		let mut runs = BTreeSet::new();
		let mut queue = VecDeque::new();
		loop {
			let page = references(
				lease,
				"memory_run_reads",
				"run_id",
				"unit_id",
				id,
				cursor,
				bounds.max_graph_visits,
			)
			.await?;
			let Some(last) = page.last().copied() else {
				break;
			};
			cursor = Some(last);
			queue.extend(page.into_iter().filter(|run| runs.insert(*run)));
			while let Some(run) = queue.pop_front() {
				discover(
					lease,
					root,
					bounds,
					Condition::any().add(contains("run", run)),
					&mut pending,
					&mut seen,
					&mut banks,
				)
				.await?;
				let mut child_cursor = None;
				loop {
					let mut query = Query::select();
					query
						.distinct()
						.column(Alias::new("run_id"))
						.from(Alias::new("authorization_run_reads"))
						.and_where(Expr::col("resource_kind").eq("run"))
						.and_where(Expr::col("resource_id").eq(Expr::value(run)))
						.and_where(Expr::col("workspace_id").eq(Expr::value(root.bank.workspace)))
						.order_by(Alias::new("run_id"), Order::Asc)
						.limit(bounds.max_graph_visits as u64);
					if let Some(cursor) = child_cursor {
						query.and_where(Expr::col("run_id").gt(Expr::value(cursor)));
					}
					let children: Vec<Uuid> =
						native::query_scalar(&query.to_string(PostgresQueryBuilder))
							.scalar_all(&mut **lease.tx())
							.await?;
					let Some(last) = children.last().copied() else {
						break;
					};
					child_cursor = Some(last);
					queue.extend(children.into_iter().filter(|child| runs.insert(*child)));
				}
			}
		}
	}
	for id in seen {
		let unit = units::load(lease, id, true)
			.await?
			.ok_or(Error::Forbidden)?;
		// Keep an independently corrected current body. All old quoted history
		// and vector generations are erased, even when their latest lineage changed.
		let withdrawn = unit.deleted
			|| unit.stale
			|| match units::current(
				lease,
				unit.bank.workspace,
				&unit.content.evidence,
				bounds.max_graph_visits,
			)
			.await
			{
				Ok(()) => false,
				Err(Error::Forbidden | Error::Conflict(_)) => true,
				Err(error) => return Err(error),
			};
		if withdrawn {
			native::query(
				&empty_body("memory_units")
					.value(Alias::new("stale"), !unit.deleted)
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			let mut cleared = unit.clone();
			cleared.content.text.clear();
			cleared.content.mental_model = None;
			cleared.content.occurred = None;
			cleared.content.entities.clear();
			cleared.content.links.clear();
			cleared.stale = !cleared.deleted;
			lease.tx().observe_memory(&cleared)?;
		}
		super::super::remote_memory_reads::erase_receipts(lease, &unit, withdrawn).await?;
		native::query(
			&Query::delete()
				.from_table(Alias::new("memory_history"))
				.and_where(Expr::col("unit_id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		let mut points = Query::select();
		points
			.column(Alias::new("id"))
			.from(Alias::new("semantic_points"))
			.and_where(Expr::col("entry_id").eq(Expr::value(id)));
		if !withdrawn {
			points.and_where(Expr::col("retired").eq(true));
		}
		native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_vectors"))
				.and_where(Expr::col("id").in_subquery(points))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	Ok(banks)
}

async fn references(
	lease: &mut Lease<'_>,
	table: &str,
	identity: &str,
	source: &str,
	id: Uuid,
	cursor: Option<Uuid>,
	limit: usize,
) -> Result<Vec<Uuid>> {
	let mut query = Query::select();
	query
		.distinct()
		.column(Alias::new(identity))
		.from(Alias::new(table))
		.and_where(Expr::col(Alias::new(source)).eq(Expr::value(id)))
		.order_by(Alias::new(identity), Order::Asc)
		.limit(limit as u64);
	if let Some(cursor) = cursor {
		query.and_where(Expr::col(Alias::new(identity)).gt(Expr::value(cursor)));
	}
	native::query_scalar(&query.to_string(PostgresQueryBuilder))
		.scalar_all(&mut **lease.tx())
		.await
}

async fn discover(
	lease: &mut Lease<'_>,
	root: &Unit,
	bounds: &Bounds,
	support: Condition,
	pending: &mut VecDeque<Uuid>,
	seen: &mut BTreeSet<Uuid>,
	banks: &mut BTreeSet<Uuid>,
) -> Result<()> {
	let workspace_banks = || {
		Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("home").eq(root.bank.home.as_str()))
			.and_where(Expr::col("tenant").eq(root.bank.tenant.as_str()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(root.bank.workspace)))
			.to_owned()
	};
	// Current lineage survives history expiry. Historical revisions also track
	// old quotations when an independent correction changes current support.
	for (table, identity) in [("memory_units", "id"), ("memory_history", "unit_id")] {
		let rows = native::query(
			&Query::select()
				.distinct()
				.expr_as(Expr::col(Alias::new(identity)), Alias::new("unit_id"))
				.column(Alias::new("bank_id"))
				.from(Alias::new(table))
				.and_where(Expr::col("bank_id").in_subquery(workspace_banks()))
				.cond_where(support.clone())
				.limit(bounds.max_graph_visits as u64 + 1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&mut **lease.tx())
		.await?;
		if rows.len() > bounds.max_graph_visits {
			return Err(Error::Conflict(
				"memory purge impact exceeds its declared bound".into(),
			));
		}
		for row in rows {
			let dependent: Uuid = row.try_get("unit_id")?;
			banks.insert(row.try_get("bank_id")?);
			if seen.insert(dependent) {
				if seen.len() > bounds.max_graph_visits {
					return Err(Error::Conflict(
						"memory purge graph exceeds its declared bound".into(),
					));
				}
				pending.push_back(dependent);
			}
		}
	}
	let candidate_rows = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_candidates"))
			.and_where(Expr::col("bank_id").in_subquery(workspace_banks()))
			.cond_where(support)
			.limit(bounds.max_graph_visits as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if candidate_rows.len() > bounds.max_graph_visits {
		return Err(Error::Conflict(
			"memory candidate purge impact exceeds its declared bound".into(),
		));
	}
	for row in candidate_rows {
		match units::current(
			lease,
			root.bank.workspace,
			&units::content(&row)?.evidence,
			bounds.max_graph_visits,
		)
		.await
		{
			Ok(()) => continue,
			Err(Error::Forbidden | Error::Conflict(_)) => {}
			Err(error) => return Err(error),
		}
		banks.insert(row.try_get("bank_id")?);
		native::query(
			&empty_body("memory_candidates")
				.value(Alias::new("state"), "invalidated")
				.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
				.and_where(Expr::col("id").eq(Expr::value(row.try_get::<Uuid>("id")?)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
	}
	Ok(())
}

fn contains(kind: &str, id: Uuid) -> reinhardt::query::SimpleExpr {
	Expr::col("evidence").binary(
		BinOper::PgOperator(PgBinOper::Contains),
		Expr::value(serde_json::json!([{"kind":kind,"id":id}])),
	)
}
fn empty_body(table: &str) -> reinhardt::query::UpdateStatement {
	Query::update()
		.table(Alias::new(table))
		.value(Alias::new("text"), "")
		.value(Alias::new("mental_model"), None::<serde_json::Value>)
		.value(
			Alias::new("occurred_start"),
			None::<chrono::DateTime<chrono::Utc>>,
		)
		.value(
			Alias::new("occurred_end"),
			None::<chrono::DateTime<chrono::Utc>>,
		)
		.value(Alias::new("entities"), serde_json::json!([]))
		.value(Alias::new("links"), serde_json::json!([]))
		.to_owned()
}

//! Erase withdrawn quotations, including old revisions and selected shared copies.
use super::super::{access::Lease, units};
use crate::{Error, Result, database::native};
use aidash_domain::memory::{Bounds, Unit};
use reinhardt::query::types::PgBinOper;
use reinhardt::query::{
	Alias, BinOper, ColumnRef, Condition, Expr, ExprTrait, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

struct Impact {
	pending: VecDeque<(Uuid, Uuid)>,
	seen: BTreeSet<Uuid>,
	graphs: BTreeMap<Uuid, BTreeSet<Uuid>>,
	banks: BTreeSet<Uuid>,
}
impl Impact {
	fn admit(&mut self, id: Uuid, bank: Uuid, cohort: Option<Uuid>, limit: usize) -> Result<()> {
		self.banks.insert(bank);
		let graph = cohort.unwrap_or(id);
		let members = self.graphs.entry(graph).or_default();
		members.insert(id);
		if members.len() > limit {
			return Err(Error::Conflict(
				"memory purge graph exceeds its declared bound".into(),
			));
		}
		if self.seen.insert(id) {
			self.pending.push_back((id, graph));
		}
		Ok(())
	}
}

pub(super) async fn erase(
	lease: &mut Lease<'_>,
	root: &Unit,
	bank: Uuid,
	bounds: &Bounds,
) -> Result<BTreeSet<Uuid>> {
	let mut impact = Impact {
		pending: VecDeque::from([(root.id, root.id)]),
		seen: BTreeSet::from([root.id]),
		graphs: BTreeMap::from([(root.id, BTreeSet::from([root.id]))]),
		banks: BTreeSet::from([bank]),
	};
	while let Some((id, graph)) = impact.pending.pop_front() {
		discover(
			lease,
			root,
			bounds,
			Condition::any().add(contains("unit", id)),
			Some(graph),
			&mut impact,
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
			discover(lease, root, bounds, support, None, &mut impact).await?;
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
					None,
					&mut impact,
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
	for id in impact.seen {
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
	Ok(impact.banks)
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
	cohort: Option<Uuid>,
	impact: &mut Impact,
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
		let mut cursor = None;
		loop {
			let mut query = Query::select();
			query
				.distinct()
				.expr_as(Expr::col(Alias::new(identity)), Alias::new("unit_id"))
				.column(Alias::new("bank_id"))
				.from(Alias::new(table))
				.and_where(Expr::col("bank_id").in_subquery(workspace_banks()))
				.cond_where(support.clone())
				.order_by(Alias::new(identity), Order::Asc)
				.limit(bounds.max_graph_visits as u64);
			if let Some(after) = cursor {
				query.and_where(Expr::col(Alias::new(identity)).gt(Expr::value(after)));
			}
			let rows = native::query(&query.to_string(PostgresQueryBuilder))
				.fetch_all(&mut **lease.tx())
				.await?;
			let Some(last) = rows.last() else {
				break;
			};
			cursor = Some(last.try_get::<Uuid>("unit_id")?);
			for row in rows {
				impact.admit(
					row.try_get("unit_id")?,
					row.try_get("bank_id")?,
					cohort,
					bounds.max_graph_visits,
				)?;
			}
		}
	}
	let mut cursor = None;
	loop {
		let mut query = Query::select();
		query
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_candidates"))
			.and_where(Expr::col("bank_id").in_subquery(workspace_banks()))
			.cond_where(support.clone())
			.order_by(Alias::new("id"), Order::Asc)
			.limit(bounds.max_graph_visits as u64);
		if let Some(after) = cursor {
			query.and_where(Expr::col("id").gt(Expr::value(after)));
		}
		let rows = native::query(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **lease.tx())
			.await?;
		let Some(last) = rows.last() else {
			break;
		};
		cursor = Some(last.try_get::<Uuid>("id")?);
		for row in rows {
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
			impact.banks.insert(row.try_get("bank_id")?);
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

//! Typed owner predicates. All edges are explicit in classification/node.rs.
use super::*;
use crate::database::classification::node::{Ownership, Root};
use reinhardt::query::{BinOper, IntoIden, SimpleExpr, types::PgBinOper};

pub(super) struct Owners<'a> {
	pub tenant: &'a str,
	pub node: &'a str,
	pub subjects: Vec<String>,
	pub retained_registrations: Vec<String>,
	pub journals: Vec<Uuid>,
	pub home_journals: Vec<Uuid>,
	pub remote_usage: Vec<(Uuid, Uuid)>,
	pub dispatches: Vec<Uuid>,
}

pub(super) fn col(table: &str, name: &str) -> SimpleExpr {
	Expr::col((Alias::new(table), Alias::new(name))).into()
}
pub(super) fn json(expr: SimpleExpr, field: &str) -> SimpleExpr {
	expr.binary(
		BinOper::PgOperator(PgBinOper::JsonGetAsText),
		Expr::value(field).into(),
	)
	.into()
}
impl Owners<'_> {
	pub fn subject(&self, table: &str, field: &str) -> SimpleExpr {
		strings(col(table, field), &self.subjects)
	}
	pub fn root(&self, table: &str, field: &str, root: Root) -> Result<SimpleExpr> {
		use Root::*;
		let (parent, key) = match root {
			Workspace => ("workspaces", "id"),
			Run => ("runs", "id"),
			Task => ("tasks", "id"),
			Area => ("core_areas", "id"),
			Request => ("generation_requests", "id"),
			Bank => ("memory_banks", "id"),
			Unit => ("memory_units", "id"),
			Entry => ("semantic_entries", "id"),
			Grant => ("authorization_remote_grants", "id"),
			Admission => ("authorization_remote_admissions", "id"),
			MemoryOperation => ("memory_model_operations", "id"),
			SemanticOperation => ("semantic_remote_operations", "id"),
			RemoteUsage => ("generation_remote_usage", "attempt_id"),
			Thread => ("channel_threads", "id"),
			Operation => ("core_operations", "id"),
		};
		Ok(Expr::from(col(table, field))
			.in_subquery(
				Query::select()
					.column(Alias::new(key))
					.from(Alias::new(parent))
					.and_where(self.predicate(parent)?)
					.to_owned(),
			)
			.into())
	}
	pub fn predicate(&self, table: &str) -> Result<SimpleExpr> {
		self.predicate_at(table, table)
	}
	pub fn predicate_at(&self, source: &str, table: &str) -> Result<SimpleExpr> {
		use Ownership::*;
		let predicate = match classification::node::ownership(source)? {
			Shared | Recovery | Audit => return Ok(Expr::value(false).into()),
			Registrations => col(table, "node")
				.eq(Expr::value(self.node))
				.and(strings(col(table, "source_node"), &self.retained_registrations).not()),
			Node(field) => col(table, field).eq(Expr::value(self.node)),
			Subject(field) => self.subject(table, field),
			Link(field, root) => self.root(table, field, root)?,
			Areas => {
				// A present Workspace always owns its area; subject fallback is only for
				// genuinely absent Workspaces. home_node/agent_id are never selectors.
				let exists: SimpleExpr = Expr::from(col(table, "workspace_id"))
					.in_subquery(
						Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("workspaces"))
							.and_where(Expr::col("tenant").eq(Expr::value(self.tenant)))
							.to_owned(),
					)
					.into();
				self.root(table, "workspace_id", Root::Workspace)?
					.or(exists.not().and(self.subject(table, "owner")))
			}
			Records => self
				.root(table, "area_id", Root::Area)?
				.or(col(table, "area_id")
					.is_null()
					.and(col(table, "kind").ne(Expr::value("thread_tombstone")))
					.and(self.subject(table, "owner")))
				.or(col(table, "area_id")
					.is_null()
					.and(col(table, "kind").eq(Expr::value("thread_tombstone")))
					.and(
						Expr::from(json(col(table, "data"), "workspace_id")).in_subquery(
							Query::select()
								.expr(Expr::col("id").cast_as(Alias::new("text")))
								.from(Alias::new("workspaces"))
								.and_where(self.predicate("workspaces")?)
								.to_owned(),
						),
					)),
			Objects => self.root(table, "area_id", Root::Area)?,
			CascadeTasks => self.root(table, "workspace_id", Root::Workspace)?,
			SemanticJournals => ids(col(table, "id"), &self.journals),
			SourceReads => Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from(Alias::new("semantic_remote_operations"))
					.and_where(
						col("semantic_remote_operations", "tenant").eq(Expr::value(self.tenant)),
					)
					.and_where(ids(
						col("semantic_remote_operations", "id"),
						&self.home_journals,
					))
					.and_where(
						col("semantic_remote_operations", "grant_id").eq(col(table, "grant_id")),
					)
					.and_where(
						col("semantic_remote_operations", "admission_id")
							.eq(col(table, "admission_id")),
					)
					.to_owned(),
			)
			.into(),
			RemoteUsageRows => {
				let values = self
					.remote_usage
					.iter()
					.map(|(request, attempt)| format!("{request}:{attempt}"))
					.collect::<Vec<_>>();
				strings(
					SimpleExpr::FunctionCall(
						"concat".into_iden(),
						vec![
							col(table, "request_id").cast_as(Alias::new("text")),
							Expr::value(":").into(),
							col(table, "attempt_id").cast_as(Alias::new("text")),
						],
					),
					&values,
				)
			}
			Finalizations => {
				let any = self.root(table, "attempt_id", Root::RemoteUsage)?;
				let other = Expr::exists(
					Query::select()
						.expr(Expr::value(1_i32))
						.from_as(
							Alias::new("generation_remote_usage"),
							Alias::new("other_usage"),
						)
						.and_where(col("other_usage", "tenant").eq(Expr::value(self.tenant)))
						.and_where(col("other_usage", "attempt_id").eq(col(table, "attempt_id")))
						.and_where(not_true(
							self.predicate_at("generation_remote_usage", "other_usage")?,
						))
						.to_owned(),
				);
				any.and(other.not())
			}
			Dispatches => ids(col(table, "attempt_id"), &self.dispatches),
			Event => {
				self.root(table, "workspace_id", Root::Workspace)?
					.or(col(table, "workspace_id")
						.is_null()
						.and(col(table, "node").eq(Expr::value(self.node))))
			}
		};
		Ok(col(table, "tenant")
			.eq(Expr::value(self.tenant))
			.and(predicate))
	}
}

/// CTIDs are local to one locked transaction; a JSON bind avoids PostgreSQL's
/// parameter-count limit. Neither CTIDs nor this snapshot survive a retry.
pub(super) fn ctids(table: &str, ids: &[String]) -> SimpleExpr {
	Expr::from(col(table, "ctid"))
		.in_subquery(
			Query::select()
				.expr(
					SimpleExpr::FunctionCall(
						"jsonb_array_elements_text".into_iden(),
						vec![Expr::value(serde_json::json!(ids)).into()],
					)
					.cast_as(Alias::new("tid")),
				)
				.to_owned(),
		)
		.into()
}

/// SQL's three-valued predicates must treat NULL as unowned/unconfirmed.
pub(super) fn not_true(expr: SimpleExpr) -> SimpleExpr {
	SimpleExpr::FunctionCall(
		"coalesce".into_iden(),
		vec![expr, Expr::value(false).into()],
	)
	.eq(Expr::value(false))
}

/// One JSON bind avoids the PostgreSQL parameter limit for large owner sets.
pub(super) fn ids(expr: SimpleExpr, values: &[Uuid]) -> SimpleExpr {
	list(expr, serde_json::json!(values), "uuid")
}
pub(super) fn strings(expr: SimpleExpr, values: &[String]) -> SimpleExpr {
	list(expr, serde_json::json!(values), "text")
}
fn list(expr: SimpleExpr, values: serde_json::Value, kind: &str) -> SimpleExpr {
	Expr::from(expr)
		.in_subquery(
			Query::select()
				.expr(
					SimpleExpr::FunctionCall(
						"jsonb_array_elements_text".into_iden(),
						vec![Expr::value(values).into()],
					)
					.cast_as(Alias::new(kind)),
				)
				.to_owned(),
		)
		.into()
}

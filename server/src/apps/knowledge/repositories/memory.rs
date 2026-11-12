//! Native query adapters keep memory writes and read checks on the inherited lease.
use super::mutations::Entries;
use crate::{Result as NativeResult, apps::knowledge::services::core as native};
use aidash_application::{
	Result,
	ports::semantic::{memory::SemanticMemoryReadSession, mutations::SemanticEntriesSession},
};
use aidash_domain::semantic::{Source, mutations::Entry};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct Memory<'a, 'scope> {
	pub entries: Entries<'a, 'scope>,
}

/// Check existence without turning an authorization denial into a source change.
pub(crate) async fn source_invalidated(
	lease: &mut super::access::Lease<'_>,
	workspace: Uuid,
	source: &Source,
) -> crate::Result<bool> {
	use crate::Error;
	Ok(match source {
		Source::Memory { .. } => false,
		// The Area gate may hide an image without deleting it. Image sources have
		// no text to disclose here, so leave the subsequent authority check to refuse them.
		Source::WorkingImage { .. } | Source::AttachmentImage { .. } => false,
		Source::Artifact { id } => crate::apps::workspaces::models::Artifact::read_in(
			&mut **lease.tx(),
			*id,
			workspace,
			true,
		)
		.await?
		.is_none(),
		Source::Message { id } => crate::apps::workspaces::models::Message::read_in(
			&mut **lease.tx(),
			*id,
			workspace,
			true,
		)
		.await?
		.is_none(),
		Source::Unit { id } => {
			let unit = match super::units::load(lease, *id, false).await {
				Ok(Some(unit)) => unit,
				Ok(None) | Err(Error::Conflict(_)) => return Ok(true),
				Err(error) => return Err(error),
			};
			if unit.bank.workspace != workspace {
				return Ok(false);
			}
			match super::units::authorize(lease, &unit.bank, "memory.read").await {
				Ok(()) => {}
				Err(Error::Forbidden) => return Ok(false),
				Err(error) => return Err(error),
			}
			if !unit.visible() {
				return Ok(true);
			}
			match super::units::unexpired(lease, &unit).await {
				Ok(policy) => match super::units::current(
					lease,
					workspace,
					&unit.content.evidence,
					policy.bounds.max_graph_visits,
				)
				.await
				{
					Ok(()) | Err(Error::Forbidden) => false,
					Err(Error::Conflict(_)) => true,
					Err(error) => return Err(error),
				},
				Err(Error::Forbidden) => false,
				Err(Error::Conflict(_)) => true,
				Err(error) => return Err(error),
			}
		}
	})
}

#[async_trait]
impl SemanticMemoryReadSession for Memory<'_, '_> {
	async fn classified_output_source(
		&mut self,
		workspace: Uuid,
		source: &Source,
		pending: &mut Vec<Uuid>,
	) -> Result<Option<String>> {
		if let Some(access) = self.entries.lease.access() {
			aidash_application::authorization::visibility::provenance::classified_semantic_output_source(
				&mut crate::bootstrap::run_visibility_scope(access), workspace, source, pending,
			).await
		} else {
			self.entries.source(workspace, source).await
		}
	}
	async fn native_invalidity(
		&mut self,
		run: Uuid,
	) -> Result<Option<aidash_domain::semantic::Failure>> {
		super::memory_reads::invalidity(self.entries.lease, run)
			.await
			.map_err(Into::into)
	}
	async fn native_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		super::memory_reads::visible(self.entries.lease, run)
			.await
			.map_err(Into::into)
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.entries.permits(entry, action).await
	}
	async fn entry_authorized(&mut self, entry: &Entry) -> Result<bool> {
		aidash_application::semantic::visibility::permits_resource(
			&mut crate::bootstrap::semantic_disclosure_scope(self.entries.lease),
			entry,
			"semantic.read",
		)
		.await
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.entries.source(workspace, source).await
	}
	async fn source_invalidated(&mut self, workspace: Uuid, source: &Source) -> Result<bool> {
		source_invalidated(self.entries.lease, workspace, source)
			.await
			.map_err(Into::into)
	}

	async fn dependencies(&mut self, run: Uuid) -> Result<Vec<(Uuid, i64)>> {
		let result: NativeResult<Vec<(Uuid, i64)>> = async {
			let dependencies: Vec<(Uuid, i64)> = {
				let query_bind_1 = run;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("entry_id"))))
						.expr(SimpleExpr::from(Expr::col(Alias::new("revision"))))
						.from(Alias::new("semantic_run_reads"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(run_id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("entry_id"))),
							Order::Asc,
						)
						.order_by_expr(
							SimpleExpr::from(Expr::col(Alias::new("revision"))),
							Order::Asc,
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["entry_id", "revision"])
				.fetch_all(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(dependencies)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		let result: NativeResult<Option<native::Entry>> = async {
			let entry: Option<native::Entry> = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(entry)
		}
		.await;
		result
			.map(|entry| entry.map(Into::into))
			.map_err(Into::into)
	}
	async fn point_digest(&mut self, point: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let digest: Option<Option<String>> = {
				let query_bind_1 = point;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("content_digest"))))
						.from(Alias::new("semantic_points"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **self.entries.lease.tx())
				.await?
			};
			Ok(digest.flatten())
		}
		.await;
		result.map_err(Into::into)
	}
}

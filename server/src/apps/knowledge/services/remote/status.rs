//! Native summary adapters keep the original HTTP schemas and authority scope.
use super::{Binding, Failure};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
	store::Store,
};
pub use aidash_domain::semantic::remote::status::{Provenance, Status};
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn provenance(
	access: &mut Access,
	node: &str,
	value: Option<Value>,
) -> Result<Option<Provenance>> {
	aidash_application::semantic::remote_status::provenance(
		&mut crate::bootstrap::semantic_status_scope(None, access, node),
		value,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn run_receipt(
	f: Federation,
	actor: Actor,
	id: Uuid,
) -> Result<Option<Provenance>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = aidash_application::semantic::remote_status::run_receipt(
		&mut crate::bootstrap::semantic_status_scope(
			Some(&f.store),
			&mut access,
			&f.config.node_id,
		),
		id,
	)
	.await
	.map_err(Into::into);
	let result = access.finish(result).await;
	if matches!(&result, Err(Error::Forbidden | Error::RemoteSemantic(_))) {
		// A denied reader cannot erase another subject's valid cache. Recheck the
		// original worker authority after releasing the reader's transaction.
		if let Ok(run) = f.store.run(id).await {
			match crate::apps::identity::services::peer::admission::worker_lease(
				&f,
				&run.metadata(),
			)
			.await
			{
				Ok(Some((access, _))) => {
					access.finish(Ok(())).await?;
				}
				Ok(None) => {}
				Err(Error::RemoteSemantic(Failure::Invalidated)) => {
					// worker_lease has durably scheduled and attempted erasure.
				}
				Err(_) => {}
			}
			if matches!(&result, Err(Error::RemoteSemantic(Failure::Invalidated)))
				&& crate::apps::knowledge::repositories::remote_memory_reads::erase_receiver(
					&f.store,
					&run.metadata(),
				)
				.await
				.is_err()
			{
				tracing::warn!(run = %id, "receiver quotation cleanup remains pending or failed");
			}
		}
	}
	result
}
pub(crate) async fn load(
	store: &Store,
	grant: Uuid,
	binding: &Binding,
	reason: Option<Failure>,
) -> Result<Status> {
	let mut status = aidash_application::semantic::remote_status::load(
		&crate::bootstrap::semantic_status_repository(store),
		grant,
		binding,
		reason,
	)
	.await
	.map_err(crate::Error::from)?;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let admissions = Query::select()
		.column(Alias::new("admission_id"))
		.from(Alias::new("semantic_remote_operations"))
		.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
		.to_owned();
	status.body_cleanup = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("run_id").in_subquery(admissions))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&store.pool)
	.await?;
	Ok(status)
}

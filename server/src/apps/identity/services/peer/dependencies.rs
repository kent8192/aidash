//! Bounded dependency traversal. A peer evaluates only its local graph and
//! returns the remaining node-qualified edges; it never calls another peer.
//! The initiating reader must have an explicit mapping at every authority.
use crate::{Result, authorization::access::Access, federation::Federation};

use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use uuid::Uuid;

#[derive(Clone)]
pub struct PeerDependencies {
	runtime: Federation,
}

#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerDependencies {
	PeerDependencies { runtime }
}

impl PeerDependencies {
	pub(crate) async fn verify(&self, headers: HeaderMap, input: Input) -> Result<Checked> {
		verify(self.runtime.clone(), headers, input).await
	}
}

pub(crate) async fn verify(f: Federation, headers: HeaderMap, input: Input) -> Result<Checked> {
	aidash_application::federation::dependencies::require_local_target(
		&input.reference,
		&f.config.node_id,
	)?;
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
	let result = aidash_application::federation::dependencies::verify_peer(
		&mut crate::bootstrap::dependency_scope(&mut access),
		&input.reference,
	)
	.await
	.map_err(Into::into);
	access.finish(result).await
}

impl Access {
	/// Append an edge even when the local graph has already visited a source.
	/// Deduplication happens at the coordinator, after each edge's authority is checked.
	pub(crate) async fn received_semantic_visible(&mut self, run: Uuid) -> Result<bool> {
		let bound: Option<(String, Uuid)> = {
			let query_bind_1 = run;
			let query_bind_2 = &self.identity.tenant;
			crate::database::native::query_as(
				&Query::select()
					.columns(["source_node", "grant_id"].map(Alias::new))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND tenant=? AND description->'semantic'->>'mode'='required_home')"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["source_node", "grant_id"])
			.fetch_optional(&mut **self.tx)
			.await?
		};
		let Some((node_id, grant_id)) = bound else {
			return Ok(true);
		};
		let reference = Reference::Grant {
			node_id,
			execution_node: self.node_id.clone(),
			grant_id,
			admission_id: run,
		};
		if let Some(pending) = &mut self.dependency_frontier {
			pending.push(reference);
			return Ok(pending.len() <= LIMIT);
		}
		self.verify_dependencies(vec![reference]).await
	}

	pub(crate) async fn verify_dependencies(&mut self, pending: Vec<Reference>) -> Result<bool> {
		let transport = crate::bootstrap::dependency_transport(self);
		aidash_application::federation::dependencies::verify_all(
			&mut crate::bootstrap::dependency_scope(self),
			&transport,
			crate::config::PROTOCOL_VERSION,
			pending,
		)
		.await
		.map_err(Into::into)
	}
}

use reinhardt::query::QueryStatementBuilder as _;

use http::HeaderMap;

use reinhardt::query::SimpleExpr;

pub(crate) use aidash_domain::federation::dependencies::{Checked, Input, LIMIT, Reference};

impl Access {
	pub(crate) async fn foreign_run_base_visible(
		&mut self,
		run: &crate::domain::RunMetadata,
	) -> Result<bool> {
		aidash_application::federation::foreign_reads::visible(
			&mut crate::bootstrap::foreign_run_read_scope(self),
			run,
		)
		.await
		.map_err(Into::into)
	}
}

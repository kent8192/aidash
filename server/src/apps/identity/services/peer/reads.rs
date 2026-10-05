//! Durable remote metadata dependencies and live checks on subsequent delivery.
use super::super::access::Access;
use crate::{
	Result,
	federation::{DiscoveredAgent, Federation},
	registry::digest,
};
use reinhardt::injectable;
use reinhardt::query::Alias;
use reinhardt::query::Expr;
use reinhardt::query::OnConflict;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::SimpleExpr;

use reinhardt::query::QueryStatementBuilder as _;

use uuid::Uuid;

impl Access {
	pub(crate) async fn track_discovery(&mut self, agents: &[DiscoveredAgent]) -> Result<()> {
		let Some(run) = self.read_run else {
			return Ok(());
		};
		let local: Vec<_> = agents
			.iter()
			.filter(|agent| agent.node_id == self.node_id)
			.map(|agent| agent.entity.clone())
			.collect();
		self.track_registry(&local).await?;
		// This independent commit precedes invocation/context persistence, so a
		// killed worker cannot retain metadata without its visibility dependency.
		let mut tx = crate::database::native::begin(&self.pool).await?;
		for agent in agents.iter().filter(|agent| agent.node_id != self.node_id) {
			let metadata = serde_json::to_value(&agent.entity)?;
			{
				let query_bind_1 = run;
				let query_bind_2 = &agent.node_id;
				let query_bind_3 = &agent.entity.id;
				let query_bind_4 = &agent.entity.version;
				let query_bind_5 = digest(&metadata);
				let query_bind_6 = metadata;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("authorization_run_remote_reads"))
						.columns([
							Alias::new("run_id"),
							Alias::new("node_id"),
							Alias::new("entry_id"),
							Alias::new("entry_version"),
							Alias::new("digest"),
							Alias::new("metadata"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns([
								"run_id",
								"node_id",
								"entry_id",
								"entry_version",
								"digest",
							])
							.do_nothing()
							.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
		}
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn remote_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		let transport = crate::bootstrap::dependency_transport(self);
		aidash_application::federation::registry_reads::reads_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			&transport,
			run,
		)
		.await
		.map_err(Into::into)
	}
}

pub(crate) use crate::apps::identity::serializers::peer_reads::VerifyInput;

use http::HeaderMap;

#[derive(Clone)]
pub struct PeerReads {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_reads(#[inject] runtime: Federation) -> PeerReads {
	PeerReads { runtime }
}

impl PeerReads {
	pub(crate) async fn verify(&self, headers: HeaderMap, input: VerifyInput) -> Result<bool> {
		let f = self.runtime.clone();
		aidash_domain::federation::registry_reads::validate(&input.references)?;
		let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let mut access = super::access(&f, node, &input.tenant, &input.subject).await?;
		let result = aidash_application::federation::registry_reads::verify(
			&mut crate::bootstrap::registry_verification_scope(&mut access, &f.config.node_id),
			&input.references,
		)
		.await
		.map_err(Into::into);
		access.finish(result).await
	}
}

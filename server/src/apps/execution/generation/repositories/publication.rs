//! Native publication and ancestor lookups keep the caller's exact transaction.
use crate::{authorization::access::Access, federation::Federation};
use aidash_application::{
	authorization::Snapshot,
	ports::generation::publication::{GenerationLive, GenerationPublication},
};
use aidash_domain::{
	generation::requests::Request,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{Expr, QueryStatementBuilder, SimpleExpr};
use serde_json::json;
pub(crate) struct NativePublication<'a> {
	pub runtime: &'a Federation,
	pub access: &'a mut Access,
}
#[async_trait]
impl GenerationPublication for NativePublication<'_> {
	fn node_id(&self) -> &str {
		self.runtime.config.deployment.as_str()
	}
	fn snapshot(&self) -> &Snapshot {
		&self.access.snapshot
	}
	fn replace_snapshot(&mut self, snapshot: Snapshot) {
		self.access.snapshot = snapshot;
	}
	async fn save_authority(&mut self, job: &Request) -> aidash_application::Result<()> {
		let access = &mut *self.access;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = access.snapshot.revision;
		let query_bind_3 = json!(access.snapshot.bundle);
		crate::database::native::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("authorization_bundles"))
				.value_expr(
					reinhardt::query::Alias::new("revision"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("document"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					reinhardt::query::Alias::new("updated_at"),
					reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = access.snapshot.revision;
		let query_bind_3 = json!(access.snapshot.bundle);
		let query_bind_4 = &job.root_subject;
		crate::database::native::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("authorization_revisions"))
				.columns([
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Alias::new("document"),
					reinhardt::query::Alias::new("actor"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
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
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}
	async fn register(&mut self, entry: &Entry) -> aidash_application::Result<()> {
		crate::registry::register_in(
			&mut self.access.tx,
			entry,
			self.runtime.config.deployment.as_str(),
			&crate::bootstrap::registry_validation_for(&self.runtime.store),
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn approve(&mut self, job: &Request, entry: &Entry) -> aidash_application::Result<()> {
		let access = &mut *self.access;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = &entry.id;
		let query_bind_3 = &entry.version;
		crate::database::native::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("authorization_catalog"))
				.columns([
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("node"),
					reinhardt::query::Alias::new("entry_id"),
					reinhardt::query::Alias::new("entry_version"),
					reinhardt::query::Alias::new("enabled"),
					reinhardt::query::Alias::new("revision"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::value(job.node.as_str().to_owned()))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.expr(reinhardt::query::Expr::cust("1"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}
	async fn approver(&mut self, job: &Request) -> aidash_application::Result<Option<String>> {
		use crate::apps::execution::generation::models::GenerationHistory;
		use reinhardt::db::orm::Model;
		Ok(GenerationHistory::objects()
			.filter(GenerationHistory::field_request_id().eq(job.id))
			.filter(GenerationHistory::field_status().eq("QUEUED".to_owned()))
			.order_by(&["-sequence"])
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(crate::Error::from)?
			.into_iter()
			.next()
			.map(|row| row.actor))
	}
	async fn may_approve(
		&mut self,
		subject: &str,
		entry: &Entry,
	) -> aidash_application::Result<bool> {
		let access = &*self.access;
		let evaluation = access.evaluation(
			subject,
			&access.resource(
				"registry",
				format!("{}/{}@{}", entry.kind, entry.id, entry.version),
				aidash_application::authorization::catalog::attributes(entry),
			),
			"catalog.approve",
		);
		Ok(access.snapshot.bundle.evaluate(&evaluation).allowed)
	}
	async fn catalog_history(
		&mut self,
		job: &Request,
		entry: &Entry,
		actor: &str,
	) -> aidash_application::Result<()> {
		let access = &mut *self.access;

		let query_bind_1 = &job.tenant;
		let query_bind_2 = &entry.id;
		let query_bind_3 = &entry.version;
		let query_bind_4 = actor;
		crate::database::native::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new(
					"authorization_catalog_history",
				))
				.columns([
					reinhardt::query::Alias::new("tenant"),
					reinhardt::query::Alias::new("node"),
					reinhardt::query::Alias::new("entry_id"),
					reinhardt::query::Alias::new("entry_version"),
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Alias::new("enabled"),
					reinhardt::query::Alias::new("actor"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::value(job.node.as_str().to_owned()))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("1"))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?;
		Ok(())
	}
}
pub(crate) struct NativeLive<'a> {
	pub access: &'a mut Access,
}
#[async_trait]
impl GenerationLive for NativeLive<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn jobs(
		&mut self,
		node: &str,
		agent: &EntityRef,
	) -> aidash_application::Result<Vec<Request>> {
		let access = &mut *self.access;
		let jobs: Vec<Request> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = node;
			let query_bind_3 = &access.subjects;
			let query_bind_4 = &agent.id;
			let query_bind_5 = &agent.version;
			crate::database::query_as(&reinhardt::query::Query::select().expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk))).from(reinhardt::query::Alias::new("generation_requests")).and_where(SimpleExpr::CustomWithExpr("((tenant = ? AND (? || '/' || node || '/agents/' || agent_id || '@' || agent_version) = ANY(?)) OR (agent_id = ? AND agent_version = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned()), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into()])).order_by_expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::Alias::new("id"))), reinhardt::query::Order::Asc).to_string(reinhardt::query::PostgresQueryBuilder)).fetch_all(&mut **access.tx).await?
		};
		for job in &jobs {
			crate::apps::execution::repositories::node_cancellations::require_generation_open(
				&mut **access.tx,
				job.id,
			)
			.await?;
		}
		Ok(jobs)
	}
	async fn policy_enabled(&mut self, job: &Request) -> aidash_application::Result<bool> {
		let access = &mut *self.access;
		let enabled: bool = {
			let query_bind_1 = &job.tenant;
			let query_bind_2 = &job.policy_id;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust(
						"CAST((spec ->> 'enabled') AS BOOLEAN)",
					))
					.from(reinhardt::query::Alias::new("generation_policies"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND id = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut **access.tx)
			.await?
		};
		Ok(enabled)
	}
}

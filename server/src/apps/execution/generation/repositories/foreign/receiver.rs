//! Receiver queries preserve their update lock, replay fence and publication transaction.
use crate::{
	Error,
	authorization::{access::Access, peer},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::generation::foreign::receiver::{
		ForeignGenerationReceiver, ForeignGenerationReceiverScope,
	},
};
use aidash_domain::{
	generation::{
		intent::{Intent, Prepared, guards::Authority},
		policy::{Policy, Spec},
		requests::{Assignment, Request},
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct NativeReceiver {
	pub federation: Federation,
}
struct Scope {
	federation: Federation,
	access: Access,
}
#[async_trait]
impl ForeignGenerationReceiver for NativeReceiver {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn describe(&self, source: &str, id: Uuid) -> Result<Intent> {
		peer::authority_request(
			&self.federation,
			source,
			"/scoped/generation/describe",
			&json!({"intent_id":id}),
		)
		.await
		.map_err(Into::into)
	}
	async fn begin(
		&self,
		source: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Box<dyn ForeignGenerationReceiverScope>> {
		let access = peer::access_mode(&self.federation, source, tenant, subject, true).await?;
		Ok(Box::new(Scope {
			federation: self.federation.clone(),
			access,
		}))
	}
}
#[async_trait]
impl ForeignGenerationReceiverScope for Scope {
	fn authority(&self) -> Authority {
		Authority {
			tenant: self.access.identity.tenant.clone(),
			credential_id: self.access.identity.credential_id,
			subjects: self.access.subjects.clone(),
		}
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn policy(&mut self, id: &str) -> Result<Policy> {
		crate::apps::execution::generation::services::policy::load(
			&mut self.access.tx,
			&self.access.identity.tenant,
			id,
			true,
		)
		.await
		.map_err(Into::into)
	}
	async fn existing(&mut self, source: &str, task: Uuid) -> Result<Option<Request>> {
		let access = &mut self.access;
		let existing: Option<Request> = {
			let query_bind_1 = source;
			let query_bind_2 = task;
			crate::database::query_as(&Query::select()
				.column(Asterisk)
				.from(Alias::new("generation_requests"))
				.and_where(SimpleExpr::CustomWithExpr("(home_node=? AND task_id=? AND status IN ('PENDING_APPROVAL','QUEUED','ACTIVE'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder))
		.fetch_optional(&mut **access.tx)
		.await.map_err(Error::from)?
		};
		Ok(existing)
	}
	async fn replayed(&mut self, source: &str, id: Uuid) -> Result<bool> {
		let access = &mut self.access;
		let replayed: bool = {
			let query_bind_1 = source;
			let query_bind_2 = id.to_string();
			sqlx::query_scalar(&Query::select()
					.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM generation_requests WHERE home_node=? AND foreign_intent->>'id'=?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
					.to_string(PostgresQueryBuilder))
			.fetch_one(&mut **access.tx)
			.await.map_err(Error::from)?
		};
		Ok(replayed)
	}
	async fn create(&mut self, intent: &Intent, policy: Policy) -> Result<Assignment> {
		aidash_application::generation::assignment::create_in(
			&mut crate::bootstrap::generation_assignment_scope(&self.federation, &mut self.access),
			&intent.task,
			policy,
			&intent.reason,
			Some(intent),
			&crate::bootstrap::registry_validation(),
		)
		.await
	}
	async fn publish(&mut self, job: &Request, spec: &Spec) -> Result<()> {
		aidash_application::generation::publication::publish(
			&mut crate::bootstrap::generation_publication_scope(&self.federation, &mut self.access),
			job,
			spec,
		)
		.await
	}
	async fn mark_prepared(&mut self, id: Uuid) -> Result<()> {
		let access = &mut self.access;
		{
			let query_bind_1 = id;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value(Alias::new("prepared"), true)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND status='QUEUED')".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(())
	}
	async fn finish(self: Box<Self>, result: Result<Prepared>) -> Result<Prepared> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}

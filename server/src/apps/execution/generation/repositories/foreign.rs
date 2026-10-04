//! Foreign-generation query adapters retain their original row locks and transaction.
use crate::{Error, authorization::access::Access, federation::Federation};
use aidash_application::{
	Result,
	ports::generation::foreign::{ForeignGenerationBinding, ForeignGenerationGuard},
};
use aidash_domain::{
	Task,
	federation::execution::Description,
	generation::{
		intent::guards::{Authority, Record},
		remote::Ancestor,
		requests::Request,
	},
	policy::Resource,
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, LockType, PostgresQueryBuilder, Query, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct StoredIntent {
	tenant: String,
	credential_id: Uuid,
	root_subject: String,
	subject_chain: Vec<String>,
	binding: Value,
	cancelled: bool,
}
impl From<StoredIntent> for Record {
	fn from(row: StoredIntent) -> Self {
		Self {
			tenant: row.tenant,
			credential_id: row.credential_id,
			root_subject: row.root_subject,
			subject_chain: row.subject_chain,
			binding: row.binding,
			cancelled: row.cancelled,
		}
	}
}
pub(crate) struct NativeForeignGuard<'a> {
	pub access: &'a mut Access,
}
#[async_trait]
impl ForeignGenerationGuard for NativeForeignGuard<'_> {
	fn authority(&self) -> Authority {
		Authority {
			tenant: self.access.identity.tenant.clone(),
			credential_id: self.access.identity.credential_id,
			subjects: self.access.subjects.clone(),
		}
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn job(&mut self, agent: &EntityRef) -> Result<Option<Request>> {
		let access = &mut *self.access;
		let job: Option<Request> = {
			let query_bind_1 = &agent.id;
			let query_bind_2 = &agent.version;
			crate::database::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_requests"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(agent_id=? AND agent_version=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(job)
	}
	async fn intent(&mut self, id: Uuid) -> Result<Option<Record>> {
		let access = &mut *self.access;
		let record: Option<StoredIntent> = {
			let query_bind_1 = id;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_intents"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(record.map(Into::into))
	}
	async fn lineage(&mut self, node: &str) -> Result<Vec<Ancestor>> {
		aidash_application::generation::reservation::lineage(
			&mut crate::bootstrap::generation_usage_authority_scope(self.access),
			node,
		)
		.await
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn active(&mut self, description: &Description, run: Uuid) -> Result<bool> {
		let access = &mut *self.access;
		let live: bool = {
			let query_bind_1 = &description.source_node;
			let query_bind_2 = description.task.id;
			let query_bind_3 = description.grant_id;
			let query_bind_4 = run;
			sqlx::query_scalar(&Query::select().expr(Expr::cust("COUNT(*)=1")).from(Alias::new("generation_requests"))
    .and_where(SimpleExpr::CustomWithExpr("(home_node=? AND task_id=? AND grant_id=? AND admission_id=? AND status='ACTIVE' AND NOT quota_released AND expires_at>CLOCK_TIMESTAMP())".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
    .to_string(PostgresQueryBuilder)).fetch_one(&mut **access.tx).await.map_err(Error::from)?
		};
		Ok(live)
	}
}
pub(crate) struct NativeForeignBinding<'a> {
	pub access: &'a mut Access,
	pub federation: Federation,
}
#[async_trait]
impl ForeignGenerationBinding for NativeForeignBinding<'_> {
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn prepared(&mut self, home: &str, task: Uuid) -> Result<Request> {
		let access = &mut *self.access;
		let job: Request = {
			let query_bind_1 = home;
			let query_bind_2 = task;
			crate::database::query_as(&Query::select()
			.column(Asterisk)
			.from(Alias::new("generation_requests"))
			.and_where(SimpleExpr::CustomWithExpr("(home_node=? AND task_id=? AND status IN ('PENDING_APPROVAL','QUEUED','ACTIVE'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder))
	.fetch_one(&mut **access.tx)
	.await.map_err(Error::from)?
		};
		Ok(job)
	}
	async fn bind(&mut self, job: Uuid, grant: Uuid, admission: Uuid) -> Result<()> {
		let access = &mut *self.access;

		{
			let query_bind_1 = job;
			let query_bind_2 = grant;
			let query_bind_3 = admission;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_requests"))
					.value_expr(
						Alias::new("grant_id"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("admission_id"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
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
	async fn activate(&mut self, job: &Request) -> Result<()> {
		crate::apps::execution::generation::services::lifecycle::transition(
			&self.federation,
			&mut self.access.tx,
			job,
			"ACTIVE",
			"generation-service",
			"bound foreign admission activated",
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
}

pub(crate) mod home;

pub(crate) mod receiver;

pub(crate) mod maintenance;

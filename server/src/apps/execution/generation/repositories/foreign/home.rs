//! Home persistence retains idempotency, shared row locks and the authority transaction.
use super::{NativeForeignGuard, StoredIntent};
use crate::{
	Error,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
		peer,
	},
	federation::Federation,
};
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::generation::foreign::{
		ForeignGenerationGuard,
		home::{HomeGenerationRepository, HomeGenerationScope},
	},
};
use aidash_domain::{
	Task,
	federation::execution::Description,
	generation::{
		intent::{
			Intent, Prepared,
			guards::{Authority, Record},
		},
		remote::Ancestor,
		requests::Request,
	},
	identity::Principal,
	policy::Resource,
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct NativeHome {
	pub federation: Federation,
	pub actor: Actor,
	pub principal: Principal,
}
struct Scope {
	access: Access,
}
impl Scope {
	fn guard(&mut self) -> NativeForeignGuard<'_> {
		NativeForeignGuard {
			access: &mut self.access,
		}
	}
}
#[async_trait]
impl HomeGenerationRepository for NativeHome {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	fn credential_id(&self) -> Option<Uuid> {
		match &self.actor {
			Actor::Operator => None,
			Actor::Subject(identity) => Some(identity.credential_id),
		}
	}
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn load(&self, id: Uuid) -> Result<Record> {
		let f = &self.federation;
		let row = {
			let query_bind_1 = id;
			sqlx::query_as::<_, StoredIntent>(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_intents"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&f.store.pool)
			.await
			.map_err(Error::from)?
		}
		.ok_or(Error::Forbidden);
		Ok(row?.into())
	}
	async fn begin(&self) -> Result<Box<dyn HomeGenerationScope>> {
		let Actor::Subject(identity) = &self.actor else {
			return Err(aidash_application::Error::Forbidden);
		};
		Ok(Box::new(Scope {
			access: Access::begin(&self.federation.store, identity).await?,
		}))
	}
	async fn begin_saved(
		&self,
		record: &Record,
		exclusive: bool,
	) -> Result<Box<dyn HomeGenerationScope>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: record.credential_id,
			tenant: record.tenant.clone(),
			subject: record.root_subject.clone(),
		};
		let access = if exclusive {
			Access::begin_exclusive(&self.federation.store, &identity).await?
		} else {
			Access::begin(&self.federation.store, &identity).await?
		};
		Ok(Box::new(Scope { access }))
	}
	async fn prepare(&self, target: &str, id: Uuid) -> Result<Prepared> {
		peer::authority_request(
			&self.federation,
			target,
			"/scoped/generation/prepare",
			&json!({"intent_id":id}),
		)
		.await
		.map_err(Into::into)
	}
}
#[async_trait]
impl ForeignGenerationGuard for Scope {
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
		self.guard().job(agent).await
	}
	async fn intent(&mut self, id: Uuid) -> Result<Option<Record>> {
		self.guard().intent(id).await
	}
	async fn lineage(&mut self, node: &str) -> Result<Vec<Ancestor>> {
		self.guard().lineage(node).await
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.guard().workspace(id).await
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.guard().task_resource(task).await
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.guard().require(resource, action).await
	}
	async fn active(&mut self, description: &Description, admission: Uuid) -> Result<bool> {
		self.guard().active(description, admission).await
	}
}
#[async_trait]
impl HomeGenerationScope for Scope {
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn snapshot(&self) -> &Snapshot {
		&self.access.snapshot
	}
	fn snapshot_mut(&mut self) -> &mut Snapshot {
		&mut self.access.snapshot
	}
	async fn inherit_task_origin(&mut self, id: Uuid) -> Result<()> {
		crate::authorization::execution::inherit_task_origin(&mut self.access, id).await?;
		Ok(())
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		self.access.task_read(id).await.map_err(Into::into)
	}
	async fn insert(&mut self, intent: &Intent) -> Result<()> {
		let access = &mut self.access;
		let identity = &access.identity;
		sqlx::query(&format!(
			"{} ON CONFLICT DO NOTHING",
			Query::insert()
				.into_table(Alias::new("generation_remote_intents"))
				.columns(
					[
						"id",
						"task_id",
						"tenant",
						"credential_id",
						"root_subject",
						"subject_chain",
						"binding",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.expr(Expr::cust("$5"))
						.expr(Expr::cust("$6"))
						.expr(Expr::cust("$7"))
						.to_owned()
				)
				.to_owned()
				.to_string(PostgresQueryBuilder)
		))
		.bind(intent.id)
		.bind(intent.task.id)
		.bind(&identity.tenant)
		.bind(identity.credential_id)
		.bind(&identity.subject)
		.bind(&access.subjects)
		.bind(json!(intent))
		.execute(&mut **access.tx)
		.await
		.map_err(Error::from)?;
		Ok(())
	}
	async fn saved(&mut self, id: Uuid) -> Result<Record> {
		let access = &mut self.access;
		let saved: StoredIntent = {
			let query_bind_1 = id;
			sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("generation_remote_intents"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(saved.into())
	}
	async fn current(&mut self, id: Uuid) -> Result<Record> {
		let access = &mut self.access;
		let current: StoredIntent = {
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
			.fetch_one(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		Ok(current.into())
	}
	async fn save_snapshot(&mut self) -> Result<()> {
		let access = &mut self.access;

		{
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = access.snapshot.revision;
			let query_bind_3 = json!(access.snapshot.bundle);
			sqlx::query(
				&Query::update()
					.table(Alias::new("authorization_bundles"))
					.value_expr(
						Alias::new("revision"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("document"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **access.tx)
			.await
			.map_err(Error::from)?
		};
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_revisions"))
				.columns(["tenant", "revision", "document", "actor"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&access.identity.tenant)
		.bind(access.snapshot.revision)
		.bind(json!(access.snapshot.bundle))
		.bind(&access.identity.subject)
		.execute(&mut **access.tx)
		.await
		.map_err(Error::from)?;
		Ok(())
	}
	async fn set_cancelled(&mut self, id: Uuid) -> Result<()> {
		let access = &mut self.access;
		{
			let query_bind_1 = id;
			sqlx::query(
				&Query::update()
					.table(Alias::new("generation_remote_intents"))
					.value(Alias::new("cancelled"), true)
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
	async fn finish(self: Box<Self>) -> Result<()> {
		self.access.finish(Ok(())).await.map_err(Into::into)
	}
	async fn abort(self: Box<Self>, error: aidash_application::Error) -> aidash_application::Error {
		self.access
			.finish::<()>(Err(error.into()))
			.await
			.expect_err("aborting an authority session cannot succeed")
			.into()
	}
}

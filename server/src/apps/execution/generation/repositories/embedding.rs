//! Original embedding lookups and ORM accounting retain both transaction boundaries.
use crate::{
	Error,
	apps::{
		execution::generation::models::{
			GenerationBudget, GenerationEmbeddingUsage, usage_reservations::EmbeddingAttempt,
		},
		identity::repositories::catalog::NativeCatalog,
	},
};
use aidash_application::{
	Result,
	ports::{
		catalog::CatalogScope,
		generation::{
			embedding::{
				GenerationEmbeddingAuthority, GenerationEmbeddingRepository,
				GenerationEmbeddingSession,
			},
			publication::GenerationLive,
		},
	},
};
use aidash_domain::{
	generation::{
		embedding::{Attempt, Origin},
		policy::Spec,
		requests::Request,
	},
	registry::EntityRef,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::{DatabaseConnection, TransactionExecutor};
use reinhardt::query::{
	Alias, ColumnRef, Expr, Order, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct NativeEmbeddingAuthority<'a> {
	pub catalog: NativeCatalog<'a>,
}
#[async_trait]
impl GenerationEmbeddingAuthority for NativeEmbeddingAuthority<'_> {
	async fn requests(&mut self, node: &str) -> Result<Vec<Request>> {
		let access = &mut *self.catalog.0;
		let jobs: Vec<Request> = {
			let query_bind_1 = &access.identity.tenant;
			let query_bind_2 = node;
			let query_bind_3 = &access.subjects;
			crate::database::query_as(&Query::select()
			.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
			.from(Alias::new("generation_requests"))
			.and_where(SimpleExpr::CustomWithExpr("(tenant = ? AND (? || '/agents/' || agent_id || '@' || agent_version) = ANY(?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), crate::database::text_array(query_bind_3.to_owned())]))
			.order_by_expr(SimpleExpr::from(Expr::col(Alias::new("id"))), Order::Asc)
			.to_string(PostgresQueryBuilder))
	.fetch_all(&mut **access.tx)
	.await?
		};

		Ok(jobs)
	}
	async fn pinned_policy(&mut self, job: &Request) -> Result<Spec> {
		let access = &mut *self.catalog.0;
		let spec: serde_json::Value = {
			let query_bind_1 = &job.tenant;
			let query_bind_2 = &job.policy_id;
			let query_bind_3 = job.policy_revision;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("spec"))))
					.from(Alias::new("generation_policy_history"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND policy_id = ? AND revision = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **access.tx)
			.await?
		};

		serde_json::from_value(spec)
			.map_err(Error::from)
			.map_err(Into::into)
	}
	fn live(&mut self) -> &mut dyn GenerationLive {
		self
	}
	fn catalog(&mut self) -> &mut dyn CatalogScope {
		&mut self.catalog
	}
}
#[async_trait]
impl GenerationLive for NativeEmbeddingAuthority<'_> {
	fn tenant(&self) -> &str {
		&self.catalog.0.identity.tenant
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>> {
		super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.jobs(node, agent)
		.await
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		super::publication::NativeLive {
			access: &mut *self.catalog.0,
		}
		.policy_enabled(job)
		.await
	}
}

pub(crate) struct NativeEmbeddingRepository {
	pub database: DatabaseConnection,
	pub node_id: String,
}
struct Session {
	transaction: Box<dyn TransactionExecutor>,
}
#[async_trait]
impl GenerationEmbeddingRepository for NativeEmbeddingRepository {
	fn node_id(&self) -> &str {
		&self.node_id
	}
	async fn begin(&self) -> Result<Box<dyn GenerationEmbeddingSession>> {
		Ok(Box::new(Session {
			transaction: self.database.begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl GenerationEmbeddingSession for Session {
	async fn charge(&mut self, request: Uuid, amount: i64) -> Result<()> {
		GenerationBudget::charge_embedding(self.transaction.as_mut(), request, amount)
			.await
			.map_err(Into::into)
	}
	async fn reserve(&mut self, request: Uuid, attempt: &Attempt) -> Result<()> {
		let (purpose, run, entry) = match attempt.origin {
			Origin::Query(run) => ("query", run, None),
			Origin::Index(entry) => ("index", None, Some(entry)),
		};
		let usage = EmbeddingAttempt {
			id: attempt.id,
			workspace: attempt.workspace,
			run,
			entry,
			purpose,
			provider: &attempt.provider,
			request_bytes: attempt.request_bytes,
			amount: attempt.amount,
		};
		GenerationEmbeddingUsage::reserve(self.transaction.as_mut(), request, &usage)
			.await
			.map_err(Into::into)
	}
	async fn refund(&mut self, request: Uuid, amount: i64) -> Result<()> {
		GenerationBudget::refund(self.transaction.as_mut(), request, amount)
			.await
			.map_err(Into::into)
	}
	async fn report(&mut self, request: Uuid, attempt: Uuid, reported: Option<i64>) -> Result<()> {
		GenerationEmbeddingUsage::report(self.transaction.as_mut(), request, attempt, reported)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}

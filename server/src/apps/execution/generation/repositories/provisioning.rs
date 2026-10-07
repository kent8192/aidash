//! Native provisioning retains authority, visibility and request-lock ownership.
use super::contracts::RequestAccess;
use crate::{
	Error,
	authorization::{Authorization, access::Access, catalog, execution, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_application::{
	authorization::Snapshot,
	ports::generation::{
		provisioning::{
			GenerationActivationScope, GenerationActivationSession, GenerationProvisionRead,
			GenerationProvisioning, GenerationTerminalSession,
		},
		publication::GenerationPublication,
	},
};
use aidash_domain::{
	generation::{policy::Policy, requests::Request},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{Expr, QueryStatementBuilder, SimpleExpr};
use serde_json::Value;

use uuid::Uuid;

pub(crate) struct NativeProvisioning {
	pub runtime: Federation,
}
struct Activation {
	runtime: Federation,
	access: Access,
}
struct Terminal {
	runtime: Federation,
	transaction: crate::database::native::Transaction,
}
struct Read {
	runtime: Federation,
	_visibility: crate::transactions::gate::ReadLease,
}

#[async_trait]
impl GenerationProvisioning for NativeProvisioning {
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn dispatch_remote(&self) -> aidash_application::Result<()> {
		crate::generation::remote::dispatch::reconcile(&self.runtime)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn reconcile_foreign(&self) -> aidash_application::Result<()> {
		crate::generation::foreign::reconcile(&self.runtime)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn begin_read(&self) -> aidash_application::Result<Box<dyn GenerationProvisionRead>> {
		let visibility = crate::transactions::gate::ReadLease::begin(&self.runtime.store).await?;
		Ok(Box::new(Read {
			runtime: self.runtime.clone(),
			_visibility: visibility,
		}))
	}
	async fn begin_activation(
		&self,
		job: &Request,
	) -> aidash_application::Result<Box<dyn GenerationActivationSession>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: job.credential_id,
			tenant: job.tenant.clone(),
			subject: job.root_subject.clone(),
		};
		let access = Access::begin_exclusive(&self.runtime.store, &identity).await?;
		Ok(Box::new(Activation {
			runtime: self.runtime.clone(),
			access,
		}))
	}
	async fn begin_terminal(
		&self,
		job: &Request,
	) -> aidash_application::Result<Box<dyn GenerationTerminalSession>> {
		let mut transaction = crate::database::native::begin(&self.runtime.store.pool).await?;
		Authorization::load_with_mode(&mut transaction, &job.tenant, true).await?;
		Ok(Box::new(Terminal {
			runtime: self.runtime.clone(),
			transaction,
		}))
	}
}

#[async_trait]
impl GenerationProvisionRead for Read {
	async fn jobs(&mut self) -> aidash_application::Result<Vec<Request>> {
		let f = &self.runtime;
		let jobs:Vec<Request>=crate::database::query_as(&reinhardt::query::Query::select().expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::table_asterisk("g")))).from_as(reinhardt::query::Alias::new("generation_requests"), reinhardt::query::Alias::new("g")).join(reinhardt::query::JoinType::LeftJoin, reinhardt::query::TableRef::table_alias(reinhardt::query::Alias::new("runs"), reinhardt::query::Alias::new("r")), reinhardt::query::Expr::cust("r.task_id = g.task_id AND r.agent_id = g.agent_id AND r.agent_version = g.agent_version")).and_where(reinhardt::query::Expr::cust("g.home_node='' AND g.status IN ('PENDING_APPROVAL', 'QUEUED', 'ACTIVE') AND (g.status = 'QUEUED' OR g.expires_at <= CLOCK_TIMESTAMP() OR r.phase IN ('COMPLETED', 'FAILED', 'CANCELLED'))")).order_by_expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col((reinhardt::query::Alias::new("g"), reinhardt::query::Alias::new("created_at")))), reinhardt::query::Order::Asc).order_by_expr(reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col((reinhardt::query::Alias::new("g"), reinhardt::query::Alias::new("id")))), reinhardt::query::Order::Asc).limit(32).to_string(reinhardt::query::PostgresQueryBuilder))
        .fetch_all(&f.store.pool).await?;
		Ok(jobs)
	}
	fn notify(&self) {
		self.runtime.notify.notify_waiters();
	}
}

impl Activation {
	fn publication(&mut self) -> super::publication::NativePublication<'_> {
		super::publication::NativePublication {
			runtime: &self.runtime,
			access: &mut self.access,
		}
	}
}
#[async_trait]
impl GenerationPublication for Activation {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn snapshot(&self) -> &Snapshot {
		&self.access.snapshot
	}
	fn replace_snapshot(&mut self, snapshot: Snapshot) {
		self.access.snapshot = snapshot;
	}
	async fn save_authority(&mut self, job: &Request) -> aidash_application::Result<()> {
		self.publication().save_authority(job).await
	}
	async fn register(&mut self, entry: &Entry) -> aidash_application::Result<()> {
		self.publication().register(entry).await
	}
	async fn approve(&mut self, job: &Request, entry: &Entry) -> aidash_application::Result<()> {
		self.publication().approve(job, entry).await
	}
	async fn catalog_history(
		&mut self,
		job: &Request,
		entry: &Entry,
	) -> aidash_application::Result<()> {
		self.publication().catalog_history(job, entry).await
	}
}
#[async_trait]
impl GenerationActivationSession for Activation {
	async fn finish(
		self: Box<Self>,
		result: aidash_application::Result<()>,
	) -> aidash_application::Result<()> {
		self.access
			.finish(result.map_err(Error::from))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl GenerationActivationScope for Activation {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> aidash_application::Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let node = self.runtime.config.node_id.clone();
		crate::apps::registry::repositories::bindings::preview(&mut *self.access.tx, &node, entry)
			.await
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	async fn load(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Request> {
		crate::generation::lifecycle::load(&mut self.access.tx, tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn visible(&mut self, job: &Request) -> aidash_application::Result<bool> {
		job.visible(&mut self.access).await.map_err(Into::into)
	}
	async fn require_request(&mut self, job: &Request) -> aidash_application::Result<()> {
		let resource = crate::generation::resource(&self.access, &job.policy_id);
		self.access
			.require(&resource, "generation.request")
			.await
			.map_err(Into::into)
	}
	async fn current_policy(&mut self, job: &Request) -> aidash_application::Result<Policy> {
		crate::generation::policy::load(&mut self.access.tx, &job.tenant, &job.policy_id, true)
			.await
			.map_err(Into::into)
	}
	async fn pinned_policy(&mut self, job: &Request) -> aidash_application::Result<Value> {
		let access = &mut self.access;
		let document: Value = {
			let query_bind_1 = &job.tenant;
			let query_bind_2 = &job.policy_id;
			let query_bind_3 = job.policy_revision;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("spec")),
					))
					.from(reinhardt::query::Alias::new("generation_policy_history"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND policy_id = ? AND revision = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut **access.tx)
			.await?
		};
		Ok(document)
	}
	async fn catalog_entry(
		&mut self,
		reference: &EntityRef,
		action: &str,
	) -> aidash_application::Result<Entry> {
		catalog::entry(&mut self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn transition(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		crate::generation::lifecycle::transition(
			&self.runtime,
			&mut self.access.tx,
			job,
			status,
			actor,
			reason,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn delegate(&mut self, task: Uuid, agent: &EntityRef) -> aidash_application::Result<()> {
		execution::delegate_in(&self.runtime, &mut self.access, task, agent)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}

#[async_trait]
impl GenerationTerminalSession for Terminal {
	async fn load(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Request> {
		crate::generation::lifecycle::load(&mut self.transaction, tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn run_phase(&mut self, job: &Request) -> aidash_application::Result<Option<String>> {
		let f = &self.runtime;
		let tx = &mut self.transaction;
		let phase: Option<String> = {
			let query_bind_1 = job.task_id;
			let query_bind_2 = &f.config.node_id;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("phase")),
					))
					.from(reinhardt::query::Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND home_node=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		};
		Ok(phase)
	}
	async fn transition(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		crate::generation::lifecycle::transition(
			&self.runtime,
			&mut self.transaction,
			job,
			status,
			actor,
			reason,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn finish(
		self: Box<Self>,
		result: aidash_application::Result<()>,
	) -> aidash_application::Result<()> {
		result?;
		self.transaction.commit().await.map_err(Into::into)
	}
}

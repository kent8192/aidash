//! Native approval adapters retain row locks, ownership updates and event atomicity.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::{
	serializers::{
		approvals::{
			ApprovalChoice as NativeChoice, ApprovalDecision as NativeDecision,
			Outbound as NativeOutbound, Revoke as NativeRevoke,
		},
		contracts::{Area, FileEntry},
	},
	services::{
		records::{self, Record as NativeRecord},
		sessions,
	},
};
use crate::{Result as NativeResult, authorization::access::Access, domain::Run, store::Store};
use aidash_application::{
	Error, Result,
	ports::capabilities::approvals::{ApprovalScope, Creation, Limits},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		approvals::{ApprovalChoice, ApprovalDecision, Outbound, Revoke},
		operations::MountedFile,
		records::Record,
	},
	policy::{Evaluation, PolicyBundle, Resource},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
	pub(crate) area: Option<&'a Area>,
	pub(crate) run: Option<Box<Run>>,
}
fn missing() -> Error {
	Error::External("approval repository scope invariant".into())
}
impl From<NativeOutbound> for Outbound {
	fn from(value: NativeOutbound) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
			url: value.url,
		}
	}
}
impl From<NativeRevoke> for Revoke {
	fn from(value: NativeRevoke) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
			expected_revision: value.expected_revision,
		}
	}
}
impl From<NativeDecision> for ApprovalDecision {
	fn from(value: NativeDecision) -> Self {
		Self {
			idempotency_key: value.idempotency_key,
			expected_revision: value.expected_revision,
			choice: match value.choice {
				NativeChoice::AllowOnce => ApprovalChoice::AllowOnce,
				NativeChoice::AllowRun => ApprovalChoice::AllowRun,
				NativeChoice::Deny => ApprovalChoice::Deny,
			},
			targets: value.targets,
			expires_at: value.expires_at,
		}
	}
}
#[async_trait]
impl ApprovalScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn credential(&self) -> Uuid {
		self.access.identity.credential_id
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) -> Vec<String> {
		std::mem::replace(&mut self.access.subjects, subjects)
	}
	fn policy_revision(&self) -> i64 {
		self.access.snapshot.revision
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn evaluation(&self, subject: &str, target: &Resource, action: &str) -> Evaluation {
		self.access.evaluation(subject, target, action)
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	fn limits(&self) -> Result<Limits> {
		let profile = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(Limits {
			admission: profile.admission,
			origins: profile.outbound_origins.clone(),
			approval_seconds: profile.approval_seconds,
			grant_seconds: profile.grant_seconds,
		})
	}
	async fn run_context(&mut self, id: Uuid) -> Result<RunMetadata> {
		let result: NativeResult<Run> = async {
			let run: Run = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&sessions::select("runs")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			}
			.ok_or_else(|| Error::NotFound("approval unavailable".into()))?;
			let workspace = self.access.workspace(run.workspace_id).await?;
			self.access.context = workspace.attributes.clone();
			Ok(run)
		}
		.await;
		let run = result?;
		let metadata = run.metadata();
		self.run = Some(Box::new(run));
		Ok(metadata)
	}
	async fn interaction_run(&mut self, id: Uuid) -> Result<RunMetadata> {
		let run = self.access.run_for_interaction(id).await?;
		let metadata = run.metadata();
		self.run = Some(Box::new(run));
		Ok(metadata)
	}
	async fn context_authority(&mut self) -> Result<()> {
		sessions::context_authority(self.access, self.run.as_deref().ok_or_else(missing)?)
			.await
			.map_err(Into::into)
	}
	async fn active_run(&mut self) -> Result<Option<Uuid>> {
		sessions::status(self.access, self.area.ok_or_else(missing)?)
			.await
			.map(|status| status.active_run_id)
			.map_err(Into::into)
	}
	async fn require(&mut self, target: &Resource, action: &str) -> Result<()> {
		self.access
			.require(target, action)
			.await
			.map_err(Into::into)
	}
	async fn load(&mut self, id: Uuid, kind: &str) -> Result<Record> {
		records::get(self.access, id, kind)
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn grants(&mut self, run: Uuid) -> Result<Vec<Record>> {
		let result: NativeResult<Vec<NativeRecord>> = async {
			let grants: Vec<NativeRecord> = {
				let query_bind_1 = &self.access.identity.tenant;
				let query_bind_2 = &self.access.identity.subject;
				let query_bind_3 = run.to_string();
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("owner"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("grant")),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("active")),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("expires_at")))
								.gt(Expr::current_timestamp()),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(data->>'run_id' = ?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.order_by(Alias::new("id"), Order::Asc)
						.limit(64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(grants)
		}
		.await;
		result
			.map(|records| records.into_iter().map(domain).collect())
			.map_err(Into::into)
	}
	async fn insert(&mut self, creation: Creation) -> Result<Record> {
		records::insert(
			self.access,
			creation.id,
			creation.area,
			creation.kind,
			creation.state,
			creation.data,
			creation.expires,
		)
		.await
		.map(domain)
		.map_err(Into::into)
	}
	async fn transfer_owner(&mut self, id: Uuid, owner: &str) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = id;
				let query_bind_2 = owner;
				sqlx::query(
					&reinhardt::query::Query::update()
						.table(Alias::new("core_records"))
						.value_expr(
							Alias::new("owner"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **self.access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut current = native(record);
		records::update(self.access, &mut current).await?;
		record.revision = current.revision;
		Ok(())
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, value)
			.await
			.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(missing)?
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read(self.access, &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}
}

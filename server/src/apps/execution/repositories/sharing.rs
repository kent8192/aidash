//! Native sharing retains the tenant advisory lock and destination admission predicates.
use super::files::Scope;
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::Area as NativeArea,
		sharing::{Recipient as NativeRecipient, Share as NativeShare},
	},
	services::{records, sessions, transfer},
};
use crate::{Error as NativeError, Result as NativeResult, authorization::access::Access};
use aidash_application::{
	Error, Result,
	ports::capabilities::sharing::{Limits, SharingScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		sessions::Area,
		sharing::{Recipient, Share},
	},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
fn missing() -> Error {
	Error::External("sharing repository scope invariant".into())
}
pub(crate) async fn serialize(access: &mut Access) -> NativeResult<()> {
	{
		let query_bind_1 = format!("core-share:{}", access.identity.tenant);
		crate::database::native::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}
impl From<NativeRecipient> for Recipient {
	fn from(v: NativeRecipient) -> Self {
		Self {
			node_id: v.node_id,
			agent_id: v.agent_id,
			agent_version: v.agent_version,
			thread_id: v.thread_id,
		}
	}
}
impl From<Recipient> for NativeRecipient {
	fn from(v: Recipient) -> Self {
		Self {
			node_id: v.node_id,
			agent_id: v.agent_id,
			agent_version: v.agent_version,
			thread_id: v.thread_id,
		}
	}
}
impl From<NativeShare> for Share {
	fn from(v: NativeShare) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			files: v.files,
			recipient: v.recipient.into(),
		}
	}
}
impl From<Share> for NativeShare {
	fn from(v: Share) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			files: v.files,
			recipient: v.recipient.into(),
		}
	}
}
#[async_trait]
impl SharingScope for Scope<'_> {
	fn node_id(&self) -> &str {
		self.store.map_or("", |store| store.node_id.as_str())
	}
	fn sharing_limits(&self) -> Result<Limits> {
		let p = &self.store.ok_or_else(missing)?.capabilities.0;
		let l = &p.limits;
		Ok(Limits {
			files: l.share_files,
			file_bytes: l.share_file_bytes,
			bytes: l.share_bytes,
			admission: p.admission,
		})
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	async fn recipient(&mut self, area: &Area, target: &Recipient) -> Result<Option<Area>> {
		let result: NativeResult<Option<NativeArea>> = async {
			let access = &mut *self.access;
			let store = self.store.ok_or(NativeError::Forbidden)?;
			Ok({
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = area.workspace_id;
				let query_bind_3 = target.thread_id;
				let query_bind_4 = &target.agent_id;
				let query_bind_5 = &store.node_id;
				let query_bind_6 = &access.identity.subject;
				crate::database::native::query_as(
					&sessions::select("core_areas")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("agent_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("home_node")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("owner"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map(|area| area.map(Into::into)).map_err(Into::into)
	}
	async fn admitted(&mut self, recipient: &Area, version: &str) -> Result<Option<Uuid>> {
		let result: NativeResult<Option<Uuid>> =
			async {
				let access = &mut *self.access;
				Ok({
					let query_bind_1 = recipient.id;
					let query_bind_2 = version;
					crate::database::native::query_scalar(
						&Query::select()
							.column((Alias::new("runs"), Alias::new("id")))
							.from(Alias::new("runs"))
							.and_where(
								Expr::col(Alias::new("agent_version"))
									.eq(Expr::value(query_bind_2.to_owned())),
							)
							.and_where(
								Expr::col(Alias::new("id")).in_subquery(
									Query::select()
										.column(Alias::new("run_id"))
										.from(Alias::new("core_runs"))
										.and_where(
											Expr::col(Alias::new("area_id"))
												.eq(Expr::value(query_bind_1.to_owned())),
										)
										.and_where(Expr::col(Alias::new("generation")).eq(
											reinhardt::query::Expr::value(recipient.generation),
										))
										.to_owned(),
								),
							)
							.limit(1)
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut **access.tx)
					.await?
				})
			}
			.await;
		result.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<aidash_domain::policy::Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn source_authority(&mut self, workspace: Uuid, constraints: &Value) -> Result<()> {
		sessions::authorize_sources(self.access, workspace, constraints)
			.await
			.map_err(Into::into)
	}
	async fn begin_received(&mut self, area: Uuid, size: u64) -> Result<()> {
		self.pending = Some(
			self.store
				.ok_or_else(missing)?
				.capabilities
				.begin_object(self.access, Some(area), "received", size)
				.await?,
		);
		Ok(())
	}
	async fn collaboration(&mut self, id: Uuid, area: Uuid, result: Value) -> Result<()> {
		records::insert(
			self.access,
			id,
			Some(area),
			"collaboration",
			"delivered",
			result,
			None,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn remote_view(&mut self, id: Uuid) -> Result<Value> {
		transfer::view(self.access, id).await.map_err(Into::into)
	}
	async fn remote_prepare(
		&mut self,
		run: &RunMetadata,
		area: &Area,
		input: Share,
		digest: &str,
	) -> Result<Value> {
		let current = self.run.ok_or_else(missing)?;
		if current.id != run.id {
			return Err(missing());
		};
		transfer::prepare(
			self.store.ok_or_else(missing)?,
			self.access,
			current,
			&area.clone().into(),
			input.into(),
			digest,
		)
		.await
		.map_err(Into::into)
	}
}

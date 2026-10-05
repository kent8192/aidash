//! Receiver primitives retain destination row locks, admitted-generation filtering and quota ownership.
use super::{
	capability_records::domain,
	transfer::{Authority, Repository, Scope},
};
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::{Area as NativeArea, FileEntry},
		records::Record as NativeRecord,
		transfer::{Chunk as NativeChunk, Requester as NativeRequester},
	},
	services::{service, sessions},
};
use crate::{Error as NativeError, Result as NativeResult, authorization::peer};
use aidash_application::{
	Error, Result,
	ports::capabilities::transfer::receiver::{ReceiverRepository, ReceiverScope},
};
use aidash_domain::capabilities::{
	operations::MountedFile,
	records::Record,
	sessions::Area,
	transfer::{Chunk, Description, Requester, receiver::MAX_RECIPIENT_VERSIONS_PER_AREA},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
fn missing() -> Error {
	Error::External("transfer receiver scope invariant".into())
}
impl From<NativeChunk> for Chunk {
	fn from(v: NativeChunk) -> Self {
		Self {
			transfer_id: v.transfer_id,
			input_digest: v.input_digest,
			file: v.file,
			offset: v.offset,
			data: v.data,
		}
	}
}
impl From<NativeRequester> for Requester {
	fn from(v: NativeRequester) -> Self {
		Self {
			tenant: v.tenant,
			subject: v.subject,
			cursor: v.cursor,
		}
	}
}
#[async_trait]
impl ReceiverRepository for Repository<'_> {
	fn node_id(&self) -> &str {
		&self.federation.config.node_id
	}
	async fn begin_peer(
		&self,
		source: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Box<dyn ReceiverScope + '_>> {
		let access = peer::access(self.federation, source, tenant, subject).await?;
		Ok(Box::new(Scope {
			store: Some(&self.federation.store),
			authority: Authority::Owned(Box::new(access)),
			pending: None,
		}))
	}
	async fn incoming(&self, id: Uuid) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let f = self.federation;
			{
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("transfer_in")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			}
			.ok_or_else(|| NativeError::NotFound("transfer unavailable".into()))
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn status_snapshot(&self, id: Uuid) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let f = self.federation;
			{
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("transfer_in")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			}
			.ok_or_else(|| NativeError::NotFound("transfer unavailable".into()))
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
}
#[async_trait]
impl ReceiverScope for Scope<'_> {
	async fn recipient(
		&mut self,
		description: &Description,
		node_id: &str,
	) -> Result<Option<Area>> {
		let result: NativeResult<Option<NativeArea>> = async {
			let access = self.authority.get_mut();
			Ok({
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = &access.identity.subject;
				let query_bind_3 = description.target.thread_id;
				let query_bind_4 = &description.target.agent_id;
				let query_bind_5 = node_id;
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
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("owner"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
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
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map(|a| a.map(Into::into)).map_err(Into::into)
	}
	async fn admitted(&mut self, area: &Area, version: &str) -> Result<Option<Uuid>> {
		let result: NativeResult<Option<Uuid>> = async {
			let access = self.authority.get_mut();
			Ok({
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				let query_bind_3 = version;
				crate::database::native::query_scalar(
					&Query::select()
						.column((Alias::new("r"), Alias::new("id")))
						.from_as(Alias::new("runs"), Alias::new("r"))
						.join(
							reinhardt::query::JoinType::InnerJoin,
							reinhardt::query::TableRef::table_alias(
								Alias::new("core_runs"),
								Alias::new("q"),
							),
							Expr::col((Alias::new("r"), Alias::new("id")))
								.equals((Alias::new("q"), Alias::new("run_id"))),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("area_id")))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("generation")))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("r"), Alias::new("agent_version")))
								.eq(Expr::value(query_bind_3.to_owned())),
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
	async fn recipient_rows(&mut self, cursor: Option<Uuid>) -> Result<Vec<Area>> {
		let result: NativeResult<Vec<NativeArea>> = async {
			let access = self.authority.get_mut();
			Ok({
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = &access.identity.subject;
				let query_bind_3 = cursor.unwrap_or(Uuid::nil());
				crate::database::native::query_as(
					&sessions::select("core_areas")
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("owner")).eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("active")),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_3.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(51)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **access.tx)
				.await?
			})
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn versions(&mut self, area: &Area) -> Result<Vec<String>> {
		let result: NativeResult<Vec<String>> = async {
			let access = self.authority.get_mut();
			Ok({
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				crate::database::native::query_scalar(
					&Query::select()
						.distinct()
						.column((Alias::new("r"), Alias::new("agent_version")))
						.from_as(Alias::new("runs"), Alias::new("r"))
						.join(
							reinhardt::query::JoinType::InnerJoin,
							reinhardt::query::TableRef::table_alias(
								Alias::new("core_runs"),
								Alias::new("q"),
							),
							Expr::col((Alias::new("q"), Alias::new("run_id")))
								.equals((Alias::new("r"), Alias::new("id"))),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("area_id")))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("generation")))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.order_by((Alias::new("r"), Alias::new("agent_version")), Order::Asc)
						.limit((MAX_RECIPIENT_VERSIONS_PER_AREA + 1) as u64)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn reserve(&mut self, bytes: i64) -> Result<()> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.reserve(self.authority.get_mut(), bytes)
			.await
			.map_err(Into::into)
	}
	async fn put_staging(&mut self, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.put(self.authority.get_mut(), None, "transfer_staging", bytes)
			.await
			.map_err(Into::into)
	}
	async fn begin_received(&mut self, area: Uuid, size: u64) -> Result<()> {
		self.pending = Some(
			self.store
				.ok_or_else(missing)?
				.capabilities
				.begin_object(self.authority.get_mut(), Some(area), "received", size)
				.await?,
		);
		Ok(())
	}
	async fn write_pending(&mut self, bytes: &[u8]) -> Result<()> {
		self.pending
			.as_mut()
			.ok_or_else(missing)?
			.write_block(bytes)
			.await
			.map_err(Into::into)
	}
	async fn finish_pending(&mut self, digest: &str) -> Result<(Uuid, String)> {
		self.pending
			.take()
			.ok_or_else(missing)?
			.finish(self.authority.get_mut(), Some(digest))
			.await
			.map_err(Into::into)
	}
	async fn read_file(&mut self, file: &MountedFile) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read(self.authority.get_mut(), &FileEntry::from(file.clone()))
			.await
			.map_err(Into::into)
	}
	async fn publish(&mut self, area: &mut Area) -> Result<()> {
		let mut row: NativeArea = area.clone().into();
		let result = service::publish(
			self.store.ok_or_else(missing)?,
			self.authority.get_mut(),
			&mut row,
		)
		.await;
		*area = row.into();
		result.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(missing)?
			.event(
				&mut self.authority.get_mut().tx,
				Some(workspace),
				kind,
				data,
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}

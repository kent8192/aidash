//! Retention holds keep File versions in the caller's authority transaction.
use crate::{
	Error as NativeError, Result as NativeResult,
	apps::execution::capabilities::serializers::contracts::Area as NativeArea,
	authorization::access::Access, store::Store,
};
use aidash_application::{
	Error, Result,
	ports::capabilities::retention::{RetentionLimits, RetentionScope},
};
use aidash_domain::{
	capabilities::{
		retention::{Holder, RetainedFile, RetentionHold},
		sessions::Area,
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, IntoValue as _, JoinType, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder as _, TableRef,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
}

fn missing() -> Error {
	Error::External("retention repository scope invariant".into())
}

struct HoldRow {
	revision: i64,
	holder_kind: String,
	holder_id: String,
	peer: Option<String>,
	expires_at: Option<DateTime<Utc>>,
	file_id: Option<Uuid>,
	path: Option<String>,
	size: Option<i64>,
	media_type: Option<String>,
}
crate::native_record!(HoldRow {
	revision,
	holder_kind,
	holder_id,
	peer,
	expires_at,
	file_id,
	path,
	size,
	media_type
});

struct FileRow {
	file_id: Uuid,
	path: String,
	size: i64,
	media_type: String,
}
crate::native_record!(FileRow {
	file_id,
	path,
	size,
	media_type
});

fn holder(row: &HoldRow) -> NativeResult<Holder> {
	match row.holder_kind.as_str() {
		"message" => row
			.holder_id
			.parse::<Uuid>()
			.map(|message_id| Holder::Message { message_id })
			.map_err(|_| NativeError::External("invalid message retention hold".into())),
		"live_preview" => row
			.holder_id
			.parse::<Uuid>()
			.map(|live_preview_id| Holder::LivePreview { live_preview_id })
			.map_err(|_| NativeError::External("invalid Live preview retention hold".into())),
		"federation" => Ok(Holder::Federation {
			peer: row
				.peer
				.clone()
				.ok_or_else(|| NativeError::External("federation hold without a peer".into()))?,
			reference_id: row.holder_id.clone(),
			expires_at: row
				.expires_at
				.ok_or_else(|| NativeError::External("federation hold without an expiry".into()))?,
		}),
		_ => Err(NativeError::External("unknown retention holder".into())),
	}
}

fn hold_matches(hold: &RetentionHold, row: &HoldRow) -> bool {
	hold.revision == row.revision
		&& hold.holder.kind() == row.holder_kind
		&& hold.holder.id() == row.holder_id
}

#[async_trait]
impl RetentionScope for Scope<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn limits(&self) -> Result<RetentionLimits> {
		let profile = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(RetentionLimits {
			working_bytes: profile.working_bytes,
			retained_bytes: profile.retained_bytes,
		})
	}
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn area(&mut self, id: Uuid) -> Result<Option<Area>> {
		let result: NativeResult<Option<NativeArea>> = async {
			let tenant = self.access.identity.tenant.clone();
			// The publication that would supersede a cited version takes the same
			// row lock, so the manifest read here cannot change under us.
			Ok(crate::database::native::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("core_areas"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await?)
		}
		.await;
		result.map(|area| area.map(Into::into)).map_err(Into::into)
	}
	async fn holds(&mut self, area: Uuid) -> Result<Vec<RetentionHold>> {
		let result: NativeResult<Vec<RetentionHold>> = async {
			let tenant = self.access.identity.tenant.clone();
			let rows: Vec<HoldRow> = crate::database::native::query_as(
				&Query::select()
					.column((Alias::new("h"), Alias::new("revision")))
					.column((Alias::new("h"), Alias::new("holder_kind")))
					.column((Alias::new("h"), Alias::new("holder_id")))
					.column((Alias::new("h"), Alias::new("peer")))
					.column((Alias::new("h"), Alias::new("expires_at")))
					.column((Alias::new("f"), Alias::new("file_id")))
					.column((Alias::new("f"), Alias::new("path")))
					.column((Alias::new("f"), Alias::new("size")))
					.column((Alias::new("f"), Alias::new("media_type")))
					.from_as(Alias::new("core_retention_holds"), Alias::new("h"))
					.join(
						JoinType::LeftJoin,
						TableRef::table_alias(Alias::new("core_retention_files"), Alias::new("f")),
						Expr::col((Alias::new("f"), Alias::new("hold_id")))
							.equals((Alias::new("h"), Alias::new("id"))),
					)
					.and_where(
						Expr::col((Alias::new("h"), Alias::new("tenant")))
							.eq(Expr::value(tenant.clone())),
					)
					.and_where(
						Expr::col((Alias::new("h"), Alias::new("area_id"))).eq(Expr::value(area)),
					)
					.order_by((Alias::new("h"), Alias::new("created_at")), Order::Asc)
					.order_by((Alias::new("h"), Alias::new("id")), Order::Asc)
					.order_by((Alias::new("f"), Alias::new("file_id")), Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			let mut holds: Vec<RetentionHold> = vec![];
			for row in &rows {
				if holds.last().is_none_or(|hold| !hold_matches(hold, row)) {
					holds.push(RetentionHold {
						tenant: tenant.clone(),
						area_id: area,
						revision: row.revision,
						holder: holder(row)?,
						files: vec![],
					});
				}
				let hold = holds.last_mut().ok_or_else(|| {
					NativeError::External("retention hold grouping invariant".into())
				})?;
				if let (Some(file_id), Some(path), Some(size), Some(media_type)) = (
					row.file_id,
					row.path.clone(),
					row.size,
					row.media_type.clone(),
				) {
					hold.files.push(RetainedFile {
						file_id,
						path,
						size: u64::try_from(size)
							.map_err(|_| NativeError::External("negative object size".into()))?,
						media_type,
					});
				}
			}
			Ok(holds)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn used_bytes(&mut self) -> Result<u64> {
		let result: NativeResult<Option<i64>> = async {
			let tenant = self.access.identity.tenant.clone();
			Ok(crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("used_bytes"))
					.from(Alias::new("core_quotas"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.access.tx)
			.await?)
		}
		.await;
		Ok(result?.map_or(0, |bytes| u64::try_from(bytes).unwrap_or(0)))
	}
	async fn reclaimable_bytes(&mut self) -> Result<u64> {
		let result: NativeResult<i64> = async {
			let tenant = self.access.identity.tenant.clone();
			Ok(crate::database::native::query_scalar(
				&Query::select()
					.expr(Expr::cust("COALESCE(SUM(core_objects.size), 0)::bigint"))
					.from(Alias::new("core_objects"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.and_where(Expr::col(Alias::new("kind")).eq(Expr::value("superseded_working")))
					.and_where(Expr::cust(
						"NOT EXISTS (SELECT 1 FROM core_retention_files f \
						 JOIN core_retention_holds h ON h.id = f.hold_id \
						 WHERE f.file_id = core_objects.id AND f.tenant = core_objects.tenant \
						 AND (h.expires_at IS NULL OR h.expires_at > CURRENT_TIMESTAMP))",
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **self.access.tx)
			.await?)
		}
		.await;
		Ok(u64::try_from(result?).unwrap_or(0))
	}
	async fn insert(&mut self, hold: &RetentionHold) -> Result<()> {
		let result: NativeResult<()> = async {
			let id = Uuid::new_v4();
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("core_retention_holds"))
					.columns(
						[
							"id",
							"tenant",
							"area_id",
							"revision",
							"holder_kind",
							"holder_id",
							"peer",
							"expires_at",
							"created_at",
						]
						.map(Alias::new),
					)
					.values(vec![
						id.into(),
						hold.tenant.clone().into(),
						hold.area_id.into(),
						hold.revision.into(),
						hold.holder.kind().into(),
						hold.holder.id().into(),
						hold.holder.peer().map(str::to_owned).into_value(),
						hold.holder.expires_at().into_value(),
						Utc::now().into_value(),
					])
					.map_err(NativeError::Invalid)?
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **self.access.tx)
			.await?;
			for file in &hold.files {
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("core_retention_files"))
						.columns(
							[
								"id",
								"hold_id",
								"file_id",
								"tenant",
								"path",
								"size",
								"media_type",
							]
							.map(Alias::new),
						)
						.values(vec![
							Uuid::new_v4().into(),
							id.into(),
							file.file_id.into(),
							hold.tenant.clone().into(),
							file.path.clone().into(),
							i64::try_from(file.size)
								.map_err(|_| NativeError::Invalid("FILE_LIMIT".into()))?
								.into(),
							file.media_type.clone().into(),
						])
						.map_err(NativeError::Invalid)?
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **self.access.tx)
				.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn release(&mut self, holder: &Holder) -> Result<u64> {
		let result: NativeResult<u64> = async {
			let mut statement = Query::delete()
				.from_table(Alias::new("core_retention_holds"))
				.and_where(
					Expr::col(Alias::new("tenant"))
						.eq(Expr::value(self.access.identity.tenant.clone())),
				)
				.and_where(Expr::col(Alias::new("holder_kind")).eq(Expr::value(holder.kind())))
				.and_where(Expr::col(Alias::new("holder_id")).eq(Expr::value(holder.id())))
				.to_owned();
			if let Some(peer) = holder.peer() {
				statement = statement
					.and_where(Expr::col(Alias::new("peer")).eq(Expr::value(peer.to_owned())))
					.to_owned();
			}
			Ok(
				crate::database::native::query(&statement.to_string(PostgresQueryBuilder))
					.execute(&mut **self.access.tx)
					.await?
					.rows_affected(),
			)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn manifest_of(
		&mut self,
		area: Uuid,
		file_id: Uuid,
	) -> Result<Option<Vec<RetainedFile>>> {
		let result: NativeResult<Option<Vec<RetainedFile>>> = async {
			let tenant = self.access.identity.tenant.clone();
			let hold: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column((Alias::new("f"), Alias::new("hold_id")))
					.from_as(Alias::new("core_retention_files"), Alias::new("f"))
					.join(
						JoinType::InnerJoin,
						TableRef::table_alias(Alias::new("core_retention_holds"), Alias::new("h")),
						Expr::col((Alias::new("h"), Alias::new("id")))
							.equals((Alias::new("f"), Alias::new("hold_id"))),
					)
					.and_where(
						Expr::col((Alias::new("f"), Alias::new("file_id")))
							.eq(Expr::value(file_id)),
					)
					.and_where(
						Expr::col((Alias::new("f"), Alias::new("tenant")))
							.eq(Expr::value(tenant.clone())),
					)
					.and_where(
						Expr::col((Alias::new("h"), Alias::new("area_id"))).eq(Expr::value(area)),
					)
					.and_where(Expr::cust(
						"(h.expires_at IS NULL OR h.expires_at > CURRENT_TIMESTAMP)",
					))
					.order_by((Alias::new("h"), Alias::new("created_at")), Order::Asc)
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.access.tx)
			.await?;
			let Some(hold) = hold else {
				return Ok(None);
			};
			let rows: Vec<FileRow> = crate::database::native::query_as(
				&Query::select()
					.column(Alias::new("file_id"))
					.column(Alias::new("path"))
					.column(Alias::new("size"))
					.column(Alias::new("media_type"))
					.from(Alias::new("core_retention_files"))
					.and_where(Expr::col(Alias::new("hold_id")).eq(Expr::value(hold)))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.order_by(Alias::new("path"), Order::Asc)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **self.access.tx)
			.await?;
			let files = rows
				.into_iter()
				.map(|row| {
					Ok(RetainedFile {
						file_id: row.file_id,
						path: row.path,
						size: u64::try_from(row.size)
							.map_err(|_| NativeError::External("negative object size".into()))?,
						media_type: row.media_type,
					})
				})
				.collect::<NativeResult<Vec<_>>>()?;
			Ok(Some(files))
		}
		.await;
		result.map_err(Into::into)
	}
}

/// Bytes the Working Area keeps beyond its manifest: held File versions that a
/// later publication superseded, each object counted once across holds.
///
/// The Working Area's `working_bytes` admission counts these on top of the
/// manifest so that a hold cannot silently overrun the Area's budget.
pub(crate) async fn retained_bytes_of(access: &mut Access, area: Uuid) -> NativeResult<u64> {
	let tenant = access.identity.tenant.clone();
	let held = Query::select()
		.column((Alias::new("o"), Alias::new("size")))
		.from_as(Alias::new("core_retention_files"), Alias::new("f"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("core_retention_holds"), Alias::new("h")),
			Expr::col((Alias::new("h"), Alias::new("id")))
				.equals((Alias::new("f"), Alias::new("hold_id"))),
		)
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("core_objects"), Alias::new("o")),
			Expr::col((Alias::new("o"), Alias::new("id")))
				.equals((Alias::new("f"), Alias::new("file_id"))),
		)
		.and_where(
			Expr::col((Alias::new("h"), Alias::new("tenant"))).eq(Expr::value(tenant.clone())),
		)
		.and_where(Expr::col((Alias::new("h"), Alias::new("area_id"))).eq(Expr::value(area)))
		.and_where(Expr::cust(
			"(h.expires_at IS NULL OR h.expires_at > CURRENT_TIMESTAMP)",
		))
		.and_where(
			Expr::col((Alias::new("o"), Alias::new("kind"))).eq(Expr::value("superseded_working")),
		)
		.distinct()
		.to_owned();
	let total: i64 = crate::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("COALESCE(SUM(size), 0)::bigint"))
			.from_subquery(held, Alias::new("held"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **access.tx)
	.await?;
	Ok(u64::try_from(total).unwrap_or(0))
}

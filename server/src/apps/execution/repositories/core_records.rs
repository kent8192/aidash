use super::sessions;
use crate::{Error, Result, authorization::access::Access};
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, IntoValue as _, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use serde_json::Value;
use uuid::Uuid;

/// Receipt rows protect every File version they still name. The foreign key
/// removes these references when the owning record is deleted.
async fn sync_receipt_references(
	tx: &mut crate::database::native::Transaction,
	record: &Record,
) -> Result<()> {
	let value = match record.kind.as_str() {
		"transfer_out" if record.data["objects_released"] == true => serde_json::json!([]),
		"transfer_out" => record.data["description"]["files"].clone(),
		"transfer_in" if record.state == "committed" => record.data["receipt"]["files"].clone(),
		"collaboration" => record.data["receipt"]["files"].clone(),
		_ => return Ok(()),
	};
	let mut files: Vec<crate::apps::execution::capabilities::serializers::contracts::FileEntry> =
		serde_json::from_value(value)?;
	files.sort_by(|a, b| a.digest.cmp(&b.digest).then(a.file_id.cmp(&b.file_id)));
	crate::database::native::query(
		&Query::delete()
			.from_table(Alias::new("core_blob_references"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&record.tenant)))
			.and_where(Expr::col(Alias::new("record_id")).eq(Expr::value(record.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	for file in files {
		let holder = format!("{}:{}", record.id, file.file_id);
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_blob_references"))
				.columns(
					[
						"id",
						"tenant",
						"digest",
						"holder_kind",
						"holder_id",
						"object_id",
						"record_id",
					]
					.map(Alias::new),
				)
				.values(vec![
					Uuid::new_v4().into(),
					record.tenant.clone().into(),
					file.digest.into(),
					"receipt".into(),
					holder.into(),
					file.file_id.into(),
					record.id.into(),
				])
				.map_err(Error::Invalid)?
				.on_conflict(
					OnConflict::columns(
						["tenant", "digest", "holder_kind", "holder_id"].map(Alias::new),
					)
					.do_nothing()
					.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
	}
	Ok(())
}

pub(crate) async fn get(access: &mut Access, id: Uuid, kind: &str) -> Result<Record> {
	{
		let query_bind_1 = id;
		let query_bind_2 = &access.identity.tenant;
		let query_bind_3 = kind;
		// Shared reads such as batched skill_list calls must not exclude each other.
		let lock = if access.shared_area {
			LockType::Share
		} else {
			LockType::Update
		};
		crate::database::native::query_as(
			&sessions::select("core_records")
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("kind"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					),
				)
				.lock(lock)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("resource unavailable".into()))
}
/// Reads many records at once. Shared, so concurrent Runs of one Agent never
/// queue on the Reference files they all read.
pub(crate) async fn get_many(access: &mut Access, ids: &[Uuid], kind: &str) -> Result<Vec<Record>> {
	if ids.is_empty() {
		return Ok(Vec::new());
	}
	crate::database::native::query_as(
		&sessions::select("core_records")
			.and_where(
				Expr::col(Alias::new("id"))
					.is_in(ids.iter().map(|id| SimpleExpr::from(Expr::value(*id)))),
			)
			.and_where(
				Expr::col(Alias::new("tenant")).eq(Expr::value(access.identity.tenant.clone())),
			)
			.and_where(Expr::col(Alias::new("kind")).eq(Expr::value(kind)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **access.tx)
	.await
}
/// Of these Reference records, the ids that can no longer be used: revoked,
/// expired or gone (ADR 0113). One `get_many` however many ids, so it adds its
/// shared lock and nothing more.
pub(crate) async fn unusable_references(
	access: &mut Access,
	ids: &[Uuid],
) -> Result<std::collections::BTreeSet<Uuid>> {
	let usable = get_many(access, ids, "reference")
		.await?
		.into_iter()
		.filter(|record| !matches!(record.state.as_str(), "revoked" | "expired"))
		.map(|record| record.id)
		.collect::<std::collections::BTreeSet<_>>();
	Ok(ids
		.iter()
		.copied()
		.filter(|id| !usable.contains(id))
		.collect())
}
/// This subject's usable reference with exactly this content, if any.
pub(crate) async fn existing_reference(
	access: &mut Access,
	digest: &str,
) -> Result<Option<Record>> {
	crate::database::native::query_as(
		&sessions::select("core_records")
			.and_where(
				Expr::col(Alias::new("tenant")).eq(Expr::value(access.identity.tenant.clone())),
			)
			.and_where(
				Expr::col(Alias::new("owner")).eq(Expr::value(access.identity.subject.clone())),
			)
			.and_where(Expr::col(Alias::new("kind")).eq(Expr::value("reference")))
			.and_where(Expr::col(Alias::new("state")).is_in(["ready", "extracting"]))
			.and_where(SimpleExpr::CustomWithExpr(
				"data->'input'->>'digest' = (?)".to_owned(),
				vec![Expr::value(digest.to_owned()).into()],
			))
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **access.tx)
	.await
}
pub(crate) async fn insert(
	access: &mut Access,
	id: Uuid,
	area: Option<Uuid>,
	kind: &str,
	state: &str,
	data: Value,
	expires_at: Option<DateTime<Utc>>,
) -> Result<Record> {
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("core_records"))
			.columns(
				[
					"id",
					"tenant",
					"owner",
					"area_id",
					"kind",
					"state",
					"data",
					"expires_at",
				]
				.map(Alias::new),
			)
			// Native NULL parameters are currently encoded as INT4 (#6631).
			// Managed values retain SQL NULL's destination-column type inference.
			.values(vec![
				id.into(),
				access.identity.tenant.clone().into(),
				access.identity.subject.clone().into(),
				area.into_value(),
				kind.into(),
				state.into(),
				data.into(),
				expires_at.into_value(),
			])
			.map_err(Error::Invalid)?
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **access.tx)
	.await?;
	let record = get(access, id, kind).await?;
	sync_receipt_references(access.tx.native_mut(), &record).await?;
	Ok(record)
}
pub(crate) async fn update(access: &mut Access, record: &mut Record) -> Result<()> {
	update_committed(&mut access.tx, record).await
}
pub(crate) async fn update_committed(
	tx: &mut crate::database::native::Transaction,
	record: &mut Record,
) -> Result<()> {
	let changed = {
		let query_bind_1 = record.id;
		let query_bind_2 = &record.state;
		let query_bind_3 = &record.data;
		let query_bind_4 = record.expires_at;
		let query_bind_5 = record.revision;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("core_records"))
				.value_expr(Alias::new("state"), Expr::value(query_bind_2.to_owned()))
				.value_expr(Alias::new("data"), Expr::value(query_bind_3.to_owned()))
				.value_expr(
					Alias::new("expires_at"),
					Expr::value(query_bind_4.to_owned()),
				)
				.value_expr(
					Alias::new("revision"),
					Expr::col(Alias::new("revision")).add(reinhardt::query::Expr::value(1)),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(
					Expr::col(Alias::new("revision")).eq(Expr::value(query_bind_5.to_owned())),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	}
	.rows_affected();
	if changed != 1 {
		return Err(Error::Conflict("RECORD_CHANGED".into()));
	}
	record.revision += 1;
	sync_receipt_references(tx, record).await?;
	Ok(())
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

pub use crate::apps::execution::capabilities::serializers::records::Record;

use reinhardt::query::SimpleExpr;

use super::{ChannelAttachment, ChannelAttachmentUploadQuery, access::Lease};
use crate::{Error, Result};
use sea_orm::sea_query::{
	Alias, Asterisk, Expr, LockType, OnConflict, PostgresQueryBuilder, Query,
};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, FromRow)]
struct StoredAttachment {
	id: Uuid,
	workspace_id: Uuid,
	uploaded_by: String,
	idempotency_key: Uuid,
	filename: String,
	media_type: String,
	sha256: String,
	size_bytes: i64,
	content: Vec<u8>,
	message_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct AttachmentLink {
	id: Uuid,
	filename: String,
	media_type: String,
	size_bytes: i64,
	message_id: Option<Uuid>,
}

impl AttachmentLink {
	fn public(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
		}
	}
}

impl StoredAttachment {
	fn public(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
		}
	}
}

pub(crate) async fn upload(
	lease: &mut Lease,
	workspace: Uuid,
	query: ChannelAttachmentUploadQuery,
	content: &[u8],
) -> Result<ChannelAttachment> {
	validate(&query, content)?;
	let uploader = lease.principal();
	let digest = format!("{:x}", Sha256::digest(content));
	let insert = Query::insert()
		.into_table(Alias::new("channel_attachments"))
		.columns([
			Alias::new("id"),
			Alias::new("workspace_id"),
			Alias::new("uploaded_by"),
			Alias::new("idempotency_key"),
			Alias::new("filename"),
			Alias::new("media_type"),
			Alias::new("sha256"),
			Alias::new("size_bytes"),
			Alias::new("content"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
			Expr::cust("$6"),
			Expr::cust("$7"),
			Expr::cust("$8"),
			Expr::cust("$9"),
		])
		.on_conflict(
			OnConflict::columns([
				Alias::new("workspace_id"),
				Alias::new("uploaded_by"),
				Alias::new("idempotency_key"),
			])
			.do_nothing()
			.to_owned(),
		)
		.returning_all()
		.to_string(PostgresQueryBuilder);
	let inserted: Option<StoredAttachment> = sqlx::query_as(&insert)
		.bind(Uuid::new_v4())
		.bind(workspace)
		.bind(&uploader)
		.bind(query.idempotency_key)
		.bind(&query.filename)
		.bind(&query.media_type)
		.bind(&digest)
		.bind(
			i64::try_from(content.len())
				.map_err(|_| Error::Invalid("attachment is too large".into()))?,
		)
		.bind(content)
		.fetch_optional(&mut **lease.tx())
		.await?;
	let attachment = if let Some(attachment) = inserted {
		attachment
	} else {
		let select = Query::select()
			.column(Asterisk)
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("uploaded_by")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::cust("$3")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder);
		let existing: StoredAttachment = sqlx::query_as(&select)
			.bind(workspace)
			.bind(&uploader)
			.bind(query.idempotency_key)
			.fetch_one(&mut **lease.tx())
			.await?;
		if existing.sha256 != digest
			|| existing.filename != query.filename
			|| existing.media_type != query.media_type
			|| existing.content != content
		{
			return Err(Error::Conflict(
				"attachment idempotency key was reused with different content or metadata".into(),
			));
		}
		existing
	};
	if attachment.workspace_id != workspace
		|| attachment.uploaded_by != uploader
		|| attachment.idempotency_key != query.idempotency_key
	{
		return Err(Error::Conflict(
			"attachment idempotency scope changed".into(),
		));
	}
	Ok(attachment.public())
}

pub(crate) async fn download(
	lease: &mut Lease,
	workspace: Uuid,
	attachment_id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	let query = Query::select()
		.column(Asterisk)
		.from(Alias::new("channel_attachments"))
		.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	let attachment: Option<StoredAttachment> = sqlx::query_as(&query)
		.bind(workspace)
		.bind(attachment_id)
		.fetch_optional(&mut **lease.tx())
		.await?;
	let attachment = attachment.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	let message = attachment
		.message_id
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	lease.message(workspace, message).await?;
	Ok((attachment.public(), attachment.content))
}

pub(crate) fn digest_ids(ids: &[Uuid]) -> Result<String> {
	let unique: HashSet<_> = ids.iter().copied().collect();
	if unique.len() != ids.len() {
		return Err(Error::Invalid("attachment ids must be unique".into()));
	}
	let mut canonical: Vec<_> = ids.iter().map(Uuid::to_string).collect();
	canonical.sort_unstable();
	Ok(format!(
		"{:x}",
		Sha256::digest(canonical.join("\n").as_bytes())
	))
}

pub(crate) async fn attach(
	lease: &mut Lease,
	workspace: Uuid,
	message: Uuid,
	ids: &[Uuid],
) -> Result<Vec<ChannelAttachment>> {
	if ids.is_empty() {
		return Ok(Vec::new());
	}
	let unique: HashSet<_> = ids.iter().copied().collect();
	if unique.len() != ids.len() {
		return Err(Error::Invalid("attachment ids must be unique".into()));
	}
	let uploader = lease.principal();
	let mut query = Query::select();
	query
		.columns([
			Alias::new("id"),
			Alias::new("filename"),
			Alias::new("media_type"),
			Alias::new("size_bytes"),
			Alias::new("message_id"),
		])
		.from(Alias::new("channel_attachments"))
		.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
		.and_where(Expr::cust("id = ANY($2)"))
		.and_where(Expr::col(Alias::new("uploaded_by")).eq(Expr::cust("$3")))
		.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
		.lock(LockType::Update);
	let sql = query.to_string(PostgresQueryBuilder);
	let records: Vec<AttachmentLink> = sqlx::query_as(&sql)
		.bind(workspace)
		.bind(ids.to_vec())
		.bind(&uploader)
		.fetch_all(&mut **lease.tx())
		.await?;
	if records.len() != ids.len() {
		return Err(Error::NotFound("attachment unavailable".into()));
	}
	if records.iter().any(|record| {
		record
			.message_id
			.is_some_and(|existing| existing != message)
	}) {
		return Err(Error::Conflict(
			"attachment is already linked to another message".into(),
		));
	}
	for id in ids {
		let update = Query::update()
			.table(Alias::new("channel_attachments"))
			.value(Alias::new("message_id"), Expr::cust("$1"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$3")))
			.and_where(Expr::col(Alias::new("uploaded_by")).eq(Expr::cust("$4")))
			.and_where(Expr::col(Alias::new("message_id")).is_null())
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update)
			.bind(message)
			.bind(workspace)
			.bind(id)
			.bind(&uploader)
			.execute(&mut **lease.tx())
			.await?;
	}
	let mut ordered = Vec::with_capacity(ids.len());
	for id in ids {
		let record = records
			.iter()
			.find(|record| record.id == *id)
			.expect("queried attachment id exists");
		ordered.push(record.public());
	}
	Ok(ordered)
}

pub(crate) async fn for_messages(
	lease: &mut Lease,
	workspace: Uuid,
	ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<ChannelAttachment>>> {
	let mut grouped = std::collections::HashMap::new();
	if ids.is_empty() {
		return Ok(grouped);
	}
	let query = Query::select()
		.columns([
			Alias::new("id"),
			Alias::new("filename"),
			Alias::new("media_type"),
			Alias::new("size_bytes"),
			Alias::new("message_id"),
		])
		.from(Alias::new("channel_attachments"))
		.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
		.and_where(Expr::cust("message_id = ANY($2)"))
		.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
		.to_string(PostgresQueryBuilder);
	let records: Vec<AttachmentLink> = sqlx::query_as(&query)
		.bind(workspace)
		.bind(ids.to_vec())
		.fetch_all(&mut **lease.tx())
		.await?;
	for record in records {
		if let Some(message) = record.message_id {
			grouped
				.entry(message)
				.or_insert_with(Vec::new)
				.push(record.public());
		}
	}
	Ok(grouped)
}

fn validate(query: &ChannelAttachmentUploadQuery, content: &[u8]) -> Result<()> {
	if content.is_empty() {
		return Err(Error::Invalid("attachment must not be empty".into()));
	}
	if content.len() > 1024 * 1024 {
		return Err(Error::Invalid("attachment exceeds 1 MiB".into()));
	}
	if query.filename.is_empty()
		|| query.filename.len() > 255
		|| query.filename == "."
		|| query.filename == ".."
		|| query.filename.contains('/')
		|| query.filename.contains('\\')
		|| query.filename.chars().any(char::is_control)
	{
		return Err(Error::Invalid("invalid attachment filename".into()));
	}
	if query.media_type.len() > 128
		|| !query.media_type.contains('/')
		|| axum::http::HeaderValue::from_str(&query.media_type).is_err()
	{
		return Err(Error::Invalid("invalid attachment media type".into()));
	}
	Ok(())
}

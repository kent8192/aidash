//! Thread Attachment images in the Workspace index (#622 part 2).
//!
//! A posted image enters the index in the transaction that posts it, under the
//! poster's authority, as Working Area images enter it under their owner Run's.
//! Indexing re-reads the original bytes and the Image Description; a match is a
//! pointer to the message and attachment and never carries the bytes.
use super::{access::Lease, mutations::Entries};
use crate::{Error, Result, database::native, store::Store};
use aidash_application::ports::semantic::mutations::SemanticEntriesSession;
use aidash_application::ports::semantic::{IndexImage, IndexImageRead};
use aidash_domain::semantic::{
	Source,
	indexing::{ImageSlots, IndexingSpec, attachment_image_key},
	mutations::Entry,
};
use chrono::Utc;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, JoinType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, TableRef,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// The attachment fields the index reads; the bytes are read separately.
#[derive(Debug)]
struct AttachmentRow {
	id: Uuid,
	message_id: Uuid,
	filename: String,
	media_type: String,
	size_bytes: i64,
	sha256: String,
	object_id: Option<Uuid>,
	content: Option<Vec<u8>>,
	description: Option<String>,
}
crate::native_record!(AttachmentRow {
	id,
	message_id,
	filename,
	media_type,
	size_bytes,
	sha256,
	object_id,
	content,
	description
});

const COLUMNS: [&str; 9] = [
	"id",
	"message_id",
	"filename",
	"media_type",
	"size_bytes",
	"sha256",
	"object_id",
	"content",
	"description",
];

/// Image attachments posted in messages of `workspace`, by ID.
async fn rows(lease: &mut Lease<'_>, workspace: Uuid, ids: &[Uuid]) -> Result<Vec<AttachmentRow>> {
	let query = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("channel_attachments"))
		.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
		.and_where(Expr::col("id").is_in(ids.iter().copied().map(Expr::value)))
		.and_where(Expr::col("message_id").is_not_null())
		.and_where(Expr::col("media_type").like("image/%"))
		.order_by("position", Order::Asc)
		.order_by("id", Order::Asc)
		.to_string(PostgresQueryBuilder);
	native::query_as(&query)
		.columns(&COLUMNS)
		.fetch_all(&mut **lease.tx())
		.await
}

/// The posted image a source names, if it still exists in that message.
async fn row(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	source: &Source,
) -> Result<Option<AttachmentRow>> {
	let Source::AttachmentImage {
		message_id,
		attachment_id,
	} = source
	else {
		return Err(Error::Forbidden);
	};
	Ok(rows(lease, workspace, &[*attachment_id])
		.await?
		.into_iter()
		.find(|row| row.message_id == *message_id))
}

/// The message text, only if the current authority may read the message the
/// image was posted in. Images are read only with their message (ADR 0108).
async fn message_text(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	message: Uuid,
) -> Result<Option<String>> {
	lease
		.source(workspace, &Source::Message { id: message })
		.await
}

/// Whether the current authority may read the message an image was posted in.
pub(crate) async fn visible(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	source: &Source,
) -> Result<bool> {
	let Source::AttachmentImage { message_id, .. } = source else {
		return Ok(false);
	};
	Ok(message_text(lease, workspace, *message_id).await?.is_some()
		&& row(lease, workspace, source).await?.is_some())
}

/// What a retrieval match may show of a posted image: where it is, its name,
/// size, digest, Image Description and the text of its message. Never bytes.
pub(crate) async fn pointer(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	source: &Source,
) -> Result<Option<Value>> {
	let Source::AttachmentImage { message_id, .. } = source else {
		return Ok(None);
	};
	let Some(text) = message_text(lease, workspace, *message_id).await? else {
		return Ok(None);
	};
	let Some(row) = row(lease, workspace, source).await? else {
		return Ok(None);
	};
	let message_text: String = text
		.chars()
		.take(aidash_application::semantic::retrieval::ATTACHMENT_MESSAGE_TEXT_CHARS)
		.collect();
	Ok(Some(json!({
		"message_id": row.message_id,
		"attachment_id": row.id,
		"name": row.filename,
		"media_type": row.media_type,
		"size": row.size_bytes,
		"digest": row.sha256,
		"description": row.description,
		"message_text": message_text,
	})))
}

async fn bytes(store: &Store, lease: &mut Lease<'_>, row: &AttachmentRow) -> Result<Vec<u8>> {
	let bytes = match (row.object_id, &row.content) {
		(Some(object), _) => {
			let access = lease.access().ok_or(Error::Forbidden)?;
			crate::apps::workspaces::repositories::media::read_attachment_object(
				store,
				access,
				object,
				&row.sha256,
				row.size_bytes,
			)
			.await?
		}
		(None, Some(content)) => content.clone(),
		(None, None) => return Err(Error::NotFound("attachment unavailable".into())),
	};
	if aidash_domain::semantic::indexing::content_digest_bytes(&bytes) != row.sha256 {
		return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
	}
	Ok(bytes)
}

/// The original image and its Image Description for one indexing attempt,
/// read under the entry's restored authority.
pub(crate) async fn read(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	source: &Source,
	max_bytes: usize,
) -> Result<Option<IndexImageRead>> {
	if !visible(lease, workspace, source).await? {
		return Ok(None);
	}
	let Some(row) = row(lease, workspace, source).await? else {
		return Ok(None);
	};
	if usize::try_from(row.size_bytes)
		.ok()
		.is_none_or(|size| size > max_bytes)
	{
		return Ok(Some(IndexImageRead::TooLarge));
	}
	let bytes = bytes(store, lease, &row).await?;
	Ok(Some(IndexImageRead::Image(IndexImage {
		media_type: row.media_type,
		bytes,
		description: row.description,
	})))
}

/// The bytes of an attachment a Run selected, read under that Run's current
/// message authority, for use as an image query (#622 decision 4).
pub(crate) async fn query(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	input: &aidash_domain::semantic::retrieval::AttachmentImageQuery,
	max_bytes: usize,
) -> Result<IndexImage> {
	let source = Source::AttachmentImage {
		message_id: input.message_id,
		attachment_id: input.attachment_id,
	};
	if !visible(lease, workspace, &source).await? {
		return Err(Error::Forbidden);
	}
	let row = row(lease, workspace, &source)
		.await?
		.ok_or(Error::Forbidden)?;
	if row.sha256 != input.expected_digest {
		return Err(Error::Conflict("ATTACHMENT_CHANGED".into()));
	}
	if usize::try_from(row.size_bytes)
		.ok()
		.is_none_or(|size| size > max_bytes)
	{
		return Err(Error::Invalid(
			"image query exceeds the configured byte limit".into(),
		));
	}
	let bytes = bytes(store, lease, &row).await?;
	Ok(IndexImage {
		media_type: row.media_type,
		bytes,
		description: None,
	})
}

/// Thread Attachment images that hold an index slot: every live entry except
/// those the image limit turned away.
pub(crate) async fn counted(tx: &mut native::Transaction, workspace: Uuid) -> Result<usize> {
	let query = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new("semantic_entries"))
		.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
		.and_where(Expr::col("key").like("attachment-image:%"))
		.and_where(Expr::col("deleted").eq(Expr::value(false)))
		.and_where(Expr::cust(
			"NOT (state = 'UNSUPPORTED' AND last_error IS NOT DISTINCT FROM 'limit')",
		))
		.to_string(PostgresQueryBuilder);
	let count: i64 = native::query_scalar(&query).scalar_one(&mut **tx).await?;
	Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// Working Area images that hold an index slot in the File publication ledger.
async fn working(tx: &mut native::Transaction, workspace: Uuid) -> Result<usize> {
	let query = Query::select()
		.column("slots")
		.from(Alias::new("semantic_image_ledgers"))
		.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
		.to_string(PostgresQueryBuilder);
	let slots: Option<Value> = native::query_scalar(&query)
		.scalar_optional(&mut **tx)
		.await?;
	let Some(slots) = slots else {
		return Ok(0);
	};
	let slots: ImageSlots = serde_json::from_value(slots)?;
	Ok(slots.entries.iter().filter(|slot| slot.assigned).count())
}

/// Enter posted images into the Workspace index. With `refresh`, also queue an
/// image already there again, because its Image Description changed; without
/// it a replayed post leaves existing entries alone. The caller's transaction
/// holds the attachments; this adds the entries under the same authority. A
/// Workspace without an enabled index, or an author who may not write to it,
/// leaves them out.
pub(crate) async fn enqueue(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	ids: &[Uuid],
	refresh: bool,
) -> Result<()> {
	if ids.is_empty() || lease.access().is_none() {
		return Ok(());
	}
	// A shared index lock keeps the generation fixed without waiting for an
	// embedding call in progress, which holds the same shared lock. Admissions
	// against the image limit serialize on their own short advisory lock.
	let Some(index) =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), workspace, false)
			.await?
	else {
		return Ok(());
	};
	let admission = Query::select()
		.expr(Expr::cust(format!(
			"pg_advisory_xact_lock(71003202, hashtext('{workspace}'::text))"
		)))
		.to_string(PostgresQueryBuilder);
	native::query(&admission).execute(&mut **lease.tx()).await?;
	let spec: IndexingSpec = serde_json::from_value(index.spec.clone())?;
	if !spec.enabled {
		return Ok(());
	}
	let authority = lease.saved()?;
	let posted = rows(lease, workspace, ids).await?;
	for row in posted {
		let key = attachment_image_key(row.id);
		let source = Source::AttachmentImage {
			message_id: row.message_id,
			attachment_id: row.id,
		};
		let mut entries = Entries { lease: &mut *lease };
		let existing = entries.by_key(workspace, &key).await?;
		if existing.as_ref().is_some_and(|old| old.deleted || !refresh) {
			continue;
		}
		let entry = Entry {
			id: existing.as_ref().map_or_else(Uuid::new_v4, |old| old.id),
			workspace_id: workspace,
			key: key.clone(),
			source: serde_json::to_value(&source)?,
			agent: None,
			metadata: json!({"name":row.filename,"media_type":row.media_type,"size":row.size_bytes}),
			revision: 1,
			point_id: Uuid::new_v4(),
			index_revision: index.revision,
			deleted: false,
			state: "PENDING".into(),
			attempts: 0,
			last_error: None,
			created_by: authority["subject"].as_str().unwrap_or_default().into(),
			updated_at: Utc::now(),
		};
		if !entries.permits(&entry, "semantic.write").await?
			|| !entries.permits(&entry, "semantic.read").await?
		{
			continue;
		}
		if let Some(old) = existing {
			// A changed Image Description needs a new point; the old one is
			// retired with the revision.
			let limited = old.state == "UNSUPPORTED" && old.last_error.as_deref() == Some("limit");
			let pending = entries
				.change(
					workspace,
					old.id,
					Uuid::new_v4(),
					index.revision,
					false,
					authority.clone(),
				)
				.await?;
			entries.schedule(&pending, &index.collection).await?;
			if limited {
				super::image_publications::mark_limit(&mut entries, workspace, &pending).await?;
			} else {
				entries
					.history(
						workspace,
						pending.id,
						pending.revision,
						"PENDING",
						"Image Description changed",
					)
					.await?;
			}
			continue;
		}
		let held = counted(lease.tx(), workspace).await? + working(lease.tx(), workspace).await?;
		let mut entries = Entries { lease: &mut *lease };
		let entry = entries.put(entry, authority.clone()).await?;
		entries.schedule(&entry, &index.collection).await?;
		if held >= spec.max_images {
			super::image_publications::mark_limit(&mut entries, workspace, &entry).await?;
		} else {
			entries
				.history(
					workspace,
					entry.id,
					entry.revision,
					"PENDING",
					"Thread Attachment posted",
				)
				.await?;
		}
	}
	Ok(())
}

/// Enter images posted before the index existed or was enabled, under the
/// authority of the person configuring it, oldest message first and at most
/// the image limit at a time.
pub(crate) async fn backfill(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	max_images: usize,
) -> Result<()> {
	if lease.access().is_none() || max_images == 0 {
		return Ok(());
	}
	let query = Query::select()
		.column(("a", "id"))
		.from_as(Alias::new("channel_attachments"), Alias::new("a"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("messages"), Alias::new("m")),
			Expr::col(("m", "id")).equals(("a", "message_id")),
		)
		.and_where(Expr::col(("a", "workspace_id")).eq(Expr::value(workspace)))
		.and_where(Expr::col(("a", "media_type")).like("image/%"))
		.and_where(Expr::cust(
			"NOT EXISTS (SELECT 1 FROM semantic_entries se WHERE se.workspace_id = a.workspace_id \
			 AND se.key = 'attachment-image:' || a.id::text)",
		))
		.order_by(("m", "created_at"), Order::Asc)
		.order_by(("a", "position"), Order::Asc)
		.order_by(("a", "id"), Order::Asc)
		.limit(u64::try_from(max_images).unwrap_or(u64::MAX))
		.to_string(PostgresQueryBuilder);
	let ids: Vec<Uuid> = native::query_scalar(&query)
		.scalar_all(&mut **lease.tx())
		.await?;
	enqueue(lease, workspace, &ids, false).await
}

/// Retire the entry of a deleted image with its attachment (#622 decision 11).
pub(crate) async fn retire(lease: &mut Lease<'_>, workspace: Uuid, attachment: Uuid) -> Result<()> {
	let Some(index) =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), workspace, false)
			.await?
	else {
		return Ok(());
	};
	let authority = lease.saved()?;
	let mut entries = Entries { lease };
	let Some(entry) = entries
		.by_key(workspace, &attachment_image_key(attachment))
		.await?
	else {
		return Ok(());
	};
	if entry.deleted {
		return Ok(());
	}
	let deleted = entries
		.change(
			workspace,
			entry.id,
			Uuid::new_v4(),
			index.revision,
			true,
			authority,
		)
		.await?;
	entries.schedule(&deleted, &index.collection).await?;
	entries
		.history(
			workspace,
			deleted.id,
			deleted.revision,
			"DELETED",
			"Thread Attachment removed",
		)
		.await?;
	Ok(())
}

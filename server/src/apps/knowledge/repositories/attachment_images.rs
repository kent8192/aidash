//! Thread Attachment images in the Thread index (#622 part 2).
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
	indexing::{ATTACHMENT_IMAGE_KEY_PREFIX, ImageSlots, IndexingSpec, attachment_image_key},
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

/// SQL that holds when semantic entry `entry` is the image entry of attachment
/// `attachment`. It matches the entry's source rather than its key, so only the
/// index's own image entries count, whatever a key looks like.
pub(crate) fn names_attachment(entry: &str, attachment: &str) -> String {
	format!(
		"{entry}.source->>'kind' = 'attachment_image' \
		 AND {entry}.source->>'attachment_id' = {attachment}.id::text"
	)
}

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

/// Image attachments posted in messages of `thread`, by ID.
async fn rows(lease: &mut Lease<'_>, thread: Uuid, ids: &[Uuid]) -> Result<Vec<AttachmentRow>> {
	let query = Query::select()
		.columns(COLUMNS.map(Alias::new))
		.from(Alias::new("channel_attachments"))
		.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
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
	thread: Uuid,
	source: &Source,
) -> Result<Option<AttachmentRow>> {
	let Source::AttachmentImage {
		message_id,
		attachment_id,
	} = source
	else {
		return Err(Error::Forbidden);
	};
	Ok(rows(lease, thread, &[*attachment_id])
		.await?
		.into_iter()
		.find(|row| row.message_id == *message_id))
}

/// The message text, only if the current authority may read the message the
/// image was posted in. Images are read only with their message (ADR 0108).
async fn message_text(
	lease: &mut Lease<'_>,
	thread: Uuid,
	message: Uuid,
) -> Result<Option<String>> {
	lease.source(thread, &Source::Message { id: message }).await
}

/// Whether the current authority may read the message an image was posted in.
pub(crate) async fn visible(lease: &mut Lease<'_>, thread: Uuid, source: &Source) -> Result<bool> {
	let Source::AttachmentImage { message_id, .. } = source else {
		return Ok(false);
	};
	Ok(message_text(lease, thread, *message_id).await?.is_some()
		&& row(lease, thread, source).await?.is_some())
}

/// What a retrieval match may show of a posted image: where it is, its name,
/// size, digest, Image Description and the text of its message. Never bytes.
pub(crate) async fn pointer(
	lease: &mut Lease<'_>,
	thread: Uuid,
	source: &Source,
) -> Result<Option<Value>> {
	let Source::AttachmentImage { message_id, .. } = source else {
		return Ok(None);
	};
	let Some(text) = message_text(lease, thread, *message_id).await? else {
		return Ok(None);
	};
	let Some(row) = row(lease, thread, source).await? else {
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
	thread: Uuid,
	source: &Source,
	max_bytes: usize,
) -> Result<Option<IndexImageRead>> {
	if !visible(lease, thread, source).await? {
		return Ok(None);
	}
	let Some(row) = row(lease, thread, source).await? else {
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
	thread: Uuid,
	input: &aidash_domain::semantic::retrieval::AttachmentImageQuery,
	max_bytes: usize,
) -> Result<IndexImage> {
	let source = Source::AttachmentImage {
		message_id: input.message_id,
		attachment_id: input.attachment_id,
	};
	if !visible(lease, thread, &source).await? {
		return Err(Error::Forbidden);
	}
	let row = row(lease, thread, &source).await?.ok_or(Error::Forbidden)?;
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
pub(crate) async fn counted(tx: &mut native::Transaction, thread: Uuid) -> Result<usize> {
	let query = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new("semantic_entries"))
		.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
		.and_where(Expr::cust("source->>'kind' = 'attachment_image'"))
		.and_where(Expr::col("deleted").eq(Expr::value(false)))
		.and_where(Expr::cust(
			"NOT (state = 'UNSUPPORTED' AND last_error IS NOT DISTINCT FROM 'limit')",
		))
		.to_string(PostgresQueryBuilder);
	let count: i64 = native::query_scalar(&query).scalar_one(&mut **tx).await?;
	Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// The authority an entry was admitted and is indexed under.
async fn saved_authority(tx: &mut native::Transaction, thread: Uuid, id: Uuid) -> Result<Value> {
	let query = Query::select()
		.column("authority")
		.from(Alias::new("semantic_entries"))
		.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.to_string(PostgresQueryBuilder);
	native::query_scalar(&query).scalar_one(&mut **tx).await
}

/// Working Area images that hold an index slot in the File publication ledger.
async fn working(tx: &mut native::Transaction, thread: Uuid) -> Result<usize> {
	let query = Query::select()
		.column("slots")
		.from(Alias::new("semantic_image_ledgers"))
		.and_where(Expr::col("thread_id").eq(Expr::value(thread)))
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

/// The advisory lock class that serializes image admissions in one Thread.
/// No other lock uses it.
const ADMISSION_LOCK: i64 = 71003250;

/// Enter posted images into the Thread index. With `refresh`, also queue an
/// image already there again, under the authority that admitted it, because
/// its Image Description changed; without it a replayed post leaves existing
/// entries alone. The caller's transaction
/// holds the attachments; this adds the entries under the same authority. A
/// Thread without an enabled index, or an author who may not write to it,
/// leaves them out. Returns how many images it entered or queued again.
pub(crate) async fn enqueue(
	lease: &mut Lease<'_>,
	thread: Uuid,
	ids: &[Uuid],
	refresh: bool,
) -> Result<usize> {
	if ids.is_empty() || lease.access().is_none() {
		return Ok(0);
	}
	// A shared index lock keeps the generation fixed without waiting for an
	// embedding call in progress, which holds the same shared lock. Admissions
	// against the image limit serialize on their own short advisory lock.
	let Some(index) =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), thread, false)
			.await?
	else {
		return Ok(0);
	};
	let admission = Query::select()
		.expr(Expr::cust(format!(
			"pg_advisory_xact_lock({ADMISSION_LOCK}, hashtext('{thread}'::text))"
		)))
		.to_string(PostgresQueryBuilder);
	native::query(&admission).execute(&mut **lease.tx()).await?;
	let spec: IndexingSpec = serde_json::from_value(index.spec.clone())?;
	if !spec.enabled {
		return Ok(0);
	}
	let authority = lease.saved()?;
	let mut entered = 0;
	let mut posted = rows(lease, thread, ids).await?;
	// Admission follows the caller's order: backfill passes the oldest message
	// first, and a post passes its own attachment order. When fewer slots
	// remain than images, the earlier ones take them.
	posted.sort_by_key(|row| ids.iter().position(|id| *id == row.id));
	for row in posted {
		let key = attachment_image_key(row.id);
		let source = Source::AttachmentImage {
			message_id: row.message_id,
			attachment_id: row.id,
		};
		let mut entries = Entries { lease: &mut *lease };
		let existing = entries.by_key(thread, &key).await?;
		if let Some(old) = existing {
			if old.deleted || !refresh {
				continue;
			}
			// A changed Image Description needs a new point; the old one is
			// retired with the revision. The entry keeps the authority that
			// admitted it, as any reindexing would: the description's writer
			// may edit it without being able to write to the index, and the
			// entry must not stay Indexed with a point for the old words.
			let saved = saved_authority(entries.lease.tx(), thread, old.id).await?;
			let limited = old.state == "UNSUPPORTED" && old.last_error.as_deref() == Some("limit");
			let pending = entries
				.change(thread, old.id, Uuid::new_v4(), index.revision, false, saved)
				.await?;
			entries.schedule(&pending, &index.collection).await?;
			if limited {
				mark_limit(&mut entries, thread, &pending).await?;
			} else {
				entries
					.history(
						thread,
						pending.id,
						pending.revision,
						"PENDING",
						"Image Description changed",
					)
					.await?;
			}
			entered += 1;
			continue;
		}
		let entry = Entry {
			id: Uuid::new_v4(),
			thread_id: thread,
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
		let held = counted(lease.tx(), thread).await? + working(lease.tx(), thread).await?;
		let mut entries = Entries { lease: &mut *lease };
		let entry = entries.put(entry, authority.clone()).await?;
		entries.schedule(&entry, &index.collection).await?;
		if held >= spec.max_images {
			mark_limit(&mut entries, thread, &entry).await?;
		} else {
			entries
				.history(
					thread,
					entry.id,
					entry.revision,
					"PENDING",
					"Thread Attachment posted",
				)
				.await?;
		}
		entered += 1;
	}
	Ok(entered)
}

/// Reapply the image limit to posted images after a reconfiguration queued
/// every live entry again. Images keep their posting order and fill the room
/// Working Area images leave; the rest stay turned away. The File publication
/// ledger then rebuilds into what remains (#622 decision 12).
pub(crate) async fn relimit(lease: &mut Lease<'_>, thread: Uuid, max_images: usize) -> Result<()> {
	let room = max_images.saturating_sub(working(lease.tx(), thread).await?);
	let query = Query::select()
		.column(("a", "id"))
		.from_as(Alias::new("semantic_entries"), Alias::new("se"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("channel_attachments"), Alias::new("a")),
			Expr::cust(format!(
				"{} AND a.thread_id = se.thread_id",
				names_attachment("se", "a")
			)),
		)
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("messages"), Alias::new("m")),
			Expr::col(("m", "id")).equals(("a", "message_id")),
		)
		.and_where(Expr::col(("se", "thread_id")).eq(Expr::value(thread)))
		.and_where(Expr::col(("se", "deleted")).eq(Expr::value(false)))
		.order_by(("m", "created_at"), Order::Asc)
		.order_by(("a", "position"), Order::Asc)
		.order_by(("a", "id"), Order::Asc)
		.to_string(PostgresQueryBuilder);
	let posted: Vec<Uuid> = native::query_scalar(&query)
		.scalar_all(&mut **lease.tx())
		.await?;
	let mut entries = Entries { lease };
	for attachment in posted.into_iter().skip(room) {
		let Some(entry) = entries
			.by_key(thread, &attachment_image_key(attachment))
			.await?
		else {
			continue;
		};
		if entry.state == "UNSUPPORTED" && entry.last_error.as_deref() == Some("limit") {
			continue;
		}
		mark_limit(&mut entries, thread, &entry).await?;
	}
	Ok(())
}

/// Enter images posted before the index existed or was enabled, under the
/// authority of the person configuring it, oldest message first, until the
/// image limit's worth have entered or none remain. Images that person may
/// not read or index stay out and are paged past, so they never hold back
/// newer ones.
pub(crate) async fn backfill(lease: &mut Lease<'_>, thread: Uuid, max_images: usize) -> Result<()> {
	const PAGE: usize = 64;
	if lease.access().is_none() || max_images == 0 {
		return Ok(());
	}
	let enabled =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), thread, false)
			.await?
			.map(|index| serde_json::from_value::<IndexingSpec>(index.spec))
			.transpose()?
			.is_some_and(|spec| spec.enabled);
	if !enabled {
		return Ok(());
	}
	let (mut entered, mut skipped) = (0, 0);
	while entered < max_images {
		let query = Query::select()
			.column(("a", "id"))
			.from_as(Alias::new("channel_attachments"), Alias::new("a"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("messages"), Alias::new("m")),
				Expr::col(("m", "id")).equals(("a", "message_id")),
			)
			.and_where(Expr::col(("a", "thread_id")).eq(Expr::value(thread)))
			.and_where(Expr::col(("a", "media_type")).like("image/%"))
			// An image with an entry, or with a retired entry still holding its
			// key, has nothing to enter.
			.and_where(Expr::cust(format!(
				"NOT EXISTS (SELECT 1 FROM semantic_entries se WHERE se.thread_id = a.thread_id \
				 AND ({} OR se.key = '{ATTACHMENT_IMAGE_KEY_PREFIX}' || a.id::text))",
				names_attachment("se", "a")
			)))
			.order_by(("m", "created_at"), Order::Asc)
			.order_by(("a", "position"), Order::Asc)
			.order_by(("a", "id"), Order::Asc)
			.limit(PAGE.min(max_images - entered) as u64)
			.offset(skipped as u64)
			.to_string(PostgresQueryBuilder);
		let ids: Vec<Uuid> = native::query_scalar(&query)
			.scalar_all(&mut **lease.tx())
			.await?;
		if ids.is_empty() {
			break;
		}
		// Entered images leave the candidate set; refused ones stay in it, in
		// front, so the next page starts after them.
		let admitted = enqueue(lease, thread, &ids, false).await?;
		entered += admitted;
		skipped += ids.len() - admitted;
	}
	Ok(())
}

/// Retire the entry of a deleted image with its attachment (#622 decision 11).
pub(crate) async fn retire(lease: &mut Lease<'_>, thread: Uuid, attachment: Uuid) -> Result<()> {
	let Some(index) =
		crate::apps::knowledge::models::SemanticIndexe::locked(&mut **lease.tx(), thread, false)
			.await?
	else {
		return Ok(());
	};
	let authority = lease.saved()?;
	let mut entries = Entries { lease };
	let Some(entry) = entries
		.by_key(thread, &attachment_image_key(attachment))
		.await?
	else {
		return Ok(());
	};
	if entry.deleted {
		return Ok(());
	}
	let deleted = entries
		.change(
			thread,
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
			thread,
			deleted.id,
			deleted.revision,
			"DELETED",
			"Thread Attachment removed",
		)
		.await?;
	Ok(())
}

async fn mark_limit(entries: &mut Entries<'_, '_>, thread: Uuid, entry: &Entry) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("semantic_entries"))
			.value("state", "UNSUPPORTED")
			.value("last_error", "limit")
			.value_expr("updated_at", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("id").eq(Expr::value(entry.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	native::query(
		&Query::update()
			.table(Alias::new("semantic_points"))
			.value("retired", true)
			.value_expr("next_attempt", Expr::cust("CLOCK_TIMESTAMP()"))
			.and_where(Expr::col("entry_id").eq(Expr::value(entry.id)))
			.and_where(Expr::col("retired").eq(Expr::value(false)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **entries.lease.tx())
	.await?;
	entries
		.history(thread, entry.id, entry.revision, "UNSUPPORTED", "limit")
		.await?;
	Ok(())
}

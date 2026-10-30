//! Native attachment reads retain position/id order and the original transaction.
use crate::{Result as NativeResult, apps::identity::services::access::Access, store::Store};
use aidash_application::{
	Result,
	ports::execution::media::{AuthorizedMediaScope, MediaAttachment, MediaAttachments},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use uuid::Uuid;

struct Attachment {
	id: Uuid,
	filename: String,
	media_type: String,
	sha256: String,
	size_bytes: i64,
	content: Option<Vec<u8>>,
	object_id: Option<Uuid>,
}
crate::native_record!(Attachment {
	id,
	filename,
	media_type,
	sha256,
	size_bytes,
	content,
	object_id
});
struct Rendition {
	sha256: String,
	size_bytes: i64,
	wav_object_id: Option<Uuid>,
	wav_sha256: Option<String>,
	wav_size_bytes: Option<i64>,
}
crate::native_record!(Rendition {
	sha256,
	size_bytes,
	wav_object_id,
	wav_sha256,
	wav_size_bytes
});
pub(crate) struct ScopedMedia<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) store: &'a Store,
}
#[async_trait]
impl MediaAttachments for ScopedMedia<'_> {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		let result: NativeResult<Vec<MediaAttachment>> = async {
			let query = Query::select()
				.columns(
					[
						"id",
						"filename",
						"media_type",
						"sha256",
						"size_bytes",
						"content",
						"object_id",
					]
					.map(Alias::new),
				)
				.from(Alias::new("channel_attachments"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
						.eq(Expr::cust("$1")),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id")))
						.eq(Expr::cust("$2")),
				)
				.order_by(Alias::new("position"), Order::Asc)
				.order_by(Alias::new("id"), Order::Asc)
				.to_string(PostgresQueryBuilder);
			let rows: Vec<Attachment> = crate::database::native::query_as(&query)
				.columns(&[
					"id",
					"filename",
					"media_type",
					"sha256",
					"size_bytes",
					"content",
					"object_id",
				])
				.bind(workspace)
				.bind(message)
				.fetch_all(&mut **self.access.tx)
				.await?;
			let mut attachments = Vec::with_capacity(rows.len());
			for row in rows {
				let content = if let Some(id) = row.object_id {
					read_attachment_object(self.store, self.access, id, &row.sha256, row.size_bytes)
						.await?
				} else {
					row.content
						.ok_or_else(|| crate::Error::NotFound("attachment unavailable".into()))?
				};
				attachments.push(MediaAttachment {
					id: row.id,
					filename: row.filename,
					media_type: row.media_type,
					sha256: row.sha256,
					size_bytes: row.size_bytes,
					content,
				});
			}
			Ok(attachments)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn wav_rendition(
		&mut self,
		workspace: Uuid,
		attachment: &MediaAttachment,
	) -> Result<Vec<u8>> {
		let result: NativeResult<Vec<u8>> = async {
			let query = Query::select()
				.columns(
					[
						"sha256",
						"size_bytes",
						"wav_object_id",
						"wav_sha256",
						"wav_size_bytes",
					]
					.map(Alias::new),
				)
				.from(Alias::new("channel_attachments"))
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(workspace)))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(attachment.id)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder);
			let row: Option<Rendition> = crate::database::native::query_as(&query)
				.columns(&[
					"sha256",
					"size_bytes",
					"wav_object_id",
					"wav_sha256",
					"wav_size_bytes",
				])
				.fetch_optional(&mut **self.access.tx)
				.await?;
			let row = row.ok_or_else(|| crate::Error::NotFound("attachment unavailable".into()))?;
			if row.sha256 != attachment.sha256 || row.size_bytes != attachment.size_bytes {
				return Err(crate::Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			if let (Some(id), Some(digest), Some(size)) =
				(row.wav_object_id, row.wav_sha256, row.wav_size_bytes)
			{
				return read_attachment_object(self.store, self.access, id, &digest, size).await;
			}
			let bytes =
				aidash_integrations::media::render_wav(&attachment.media_type, &attachment.content)
					.map_err(|error| match error {
						aidash_integrations::media::Refusal::Undecodable => {
							crate::Error::Invalid("MEDIA_UNDECODABLE".into())
						}
						aidash_integrations::media::Refusal::TooLarge => {
							crate::Error::Invalid("MEDIA_TOO_LARGE".into())
						}
					})?;
			let (id, digest) = self
				.store
				.capabilities
				.put(self.access, None, "thread_attachment_wav", &bytes)
				.await?;
			let size = i64::try_from(bytes.len())
				.map_err(|_| crate::Error::Invalid("MEDIA_TOO_LARGE".into()))?;
			let update = Query::update()
				.table(Alias::new("channel_attachments"))
				.value(Alias::new("wav_object_id"), id)
				.value(Alias::new("wav_sha256"), digest)
				.value(Alias::new("wav_size_bytes"), size)
				.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(workspace)))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(attachment.id)))
				.to_string(PostgresQueryBuilder);
			crate::database::native::query(&update)
				.execute(&mut **self.access.tx)
				.await?;
			Ok(bytes)
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl AuthorizedMediaScope for ScopedMedia<'_> {
	async fn authorize_message(&mut self, workspace: Uuid, message: Uuid) -> Result<()> {
		self.access
			.workspace_record(workspace, "message", message)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
pub(crate) async fn read_attachment_object(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	digest: &str,
	size: i64,
) -> NativeResult<Vec<u8>> {
	let size =
		u64::try_from(size).map_err(|_| crate::Error::Conflict("OBJECT_INTEGRITY".into()))?;
	let entry = crate::apps::execution::capabilities::contracts::FileEntry {
		file_id: id,
		path: String::new(),
		digest: digest.to_owned(),
		size,
		media_type: String::new(),
		scope: crate::apps::execution::capabilities::contracts::FileScope::Working,
		provenance: serde_json::Value::Null,
	};
	store.capabilities.read(access, &entry).await
}

use crate::apps::execution::capabilities::{contracts::FileEntry, service, sessions};
use aidash_domain::{Run, media::SelectedFile};
use std::sync::Arc;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

pub(crate) struct SelectedMedia<'a> {
	pub(crate) store: &'a Store,
	pub(crate) run: &'a Run,
	pub(crate) access: Arc<RwLock<Access>>,
	lease: Option<OwnedRwLockWriteGuard<Access>>,
	files: Vec<FileEntry>,
}
impl<'a> SelectedMedia<'a> {
	pub(crate) fn new(store: &'a Store, run: &'a Run, access: Arc<RwLock<Access>>) -> Self {
		Self {
			store,
			run,
			access,
			lease: None,
			files: vec![],
		}
	}
}
#[async_trait]
impl aidash_application::ports::execution::media::SelectedMediaScope for SelectedMedia<'_> {
	async fn current_files(&mut self) -> Result<Vec<SelectedFile>> {
		if self.lease.is_none() {
			self.lease = Some(self.access.clone().write_owned().await);
		}
		let access = self
			.lease
			.as_mut()
			.ok_or(aidash_application::Error::Forbidden)?;
		let area = sessions::for_run(access, self.run).await?;
		sessions::require_current_run(access, &area, self.run).await?;
		sessions::authorize(access, &area, "file.read").await?;
		let files = service::files(&area)?;
		let selected = files
			.iter()
			.map(|file| SelectedFile {
				file_id: file.file_id,
				path: file.path.clone(),
				digest: file.digest.clone(),
				size: file.size,
				media_type: file.media_type.clone(),
			})
			.collect();
		self.files = files;
		Ok(selected)
	}
	async fn read(&mut self, file: Uuid) -> Result<Vec<u8>> {
		let access = self
			.lease
			.as_mut()
			.ok_or(aidash_application::Error::Forbidden)?;
		let file = self
			.files
			.iter()
			.find(|entry| entry.file_id == file)
			.ok_or_else(|| crate::Error::NotFound("file unavailable".into()))?;
		self.store
			.capabilities
			.read(access, file)
			.await
			.map_err(Into::into)
	}
	async fn page_image(&mut self, reference: &str) -> Result<Option<(String, Vec<u8>)>> {
		use aidash_application::ports::research::EvidenceStore;
		// A page image is the Run's own evidence; it needs no file access.
		let Ok(reference) = reference.parse() else {
			return Ok(None);
		};
		let scope = match &self.lease {
			Some(access) => access.tenancy()?,
			None => self.access.read().await.tenancy()?,
		};
		crate::apps::execution::repositories::research::Evidence {
			store: self.store.clone(),
			scope,
		}
		.page_image(self.run.id, &reference)
		.await
	}
}

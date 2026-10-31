//! Native attachment reads retain position/id order and the original transaction.
use crate::{Result as NativeResult, apps::identity::services::access::Access, store::Store};
use aidash_application::{
	Result,
	ports::execution::media::{AuthorizedMediaScope, MediaAttachment, MediaAttachments},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use uuid::Uuid;

struct Attachment {
	filename: String,
	media_type: String,
	sha256: String,
	size_bytes: i64,
	content: Vec<u8>,
}
crate::native_record!(Attachment {
	filename,
	media_type,
	sha256,
	size_bytes,
	content
});

pub(crate) struct Attachments<'a> {
	pub(crate) tx: &'a mut crate::database::native::Transaction,
}
pub(crate) struct ScopedMedia<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl MediaAttachments for Attachments<'_> {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		let result: NativeResult<Vec<MediaAttachment>> = async {
			let query = Query::select()
				.columns(
					["filename", "media_type", "sha256", "size_bytes", "content"].map(Alias::new),
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
			let attachments: Vec<Attachment> = crate::database::native::query_as(&query)
				.columns(&["filename", "media_type", "sha256", "size_bytes", "content"])
				.bind(workspace)
				.bind(message)
				.fetch_all(&mut **self.tx)
				.await?;
			Ok(attachments
				.into_iter()
				.map(|row| MediaAttachment {
					filename: row.filename,
					media_type: row.media_type,
					sha256: row.sha256,
					size_bytes: row.size_bytes,
					content: row.content,
				})
				.collect())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl MediaAttachments for ScopedMedia<'_> {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		Attachments {
			tx: &mut self.access.tx,
		}
		.attachments(workspace, message)
		.await
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
		// A Run without its own Working Area can still open authorized
		// Workspace image matches; it simply has no own Files to select.
		let Some(area) = sessions::current_own_area(access, self.run).await? else {
			self.files = Vec::new();
			return Ok(Vec::new());
		};
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
	async fn workspace_image(
		&mut self,
		area_id: Uuid,
		generation: i64,
		file_id: Uuid,
		path: &str,
	) -> Result<Option<SelectedFile>> {
		let access = self
			.lease
			.as_mut()
			.ok_or(aidash_application::Error::Forbidden)?;
		let area = match sessions::load(access, area_id).await {
			Ok(area) => area,
			Err(crate::Error::Forbidden | crate::Error::NotFound(_)) => return Ok(None),
			Err(error) => return Err(error.into()),
		};
		if area.workspace_id != self.run.workspace_id || area.generation != generation {
			return Ok(None);
		}
		let files = service::files(&area)?;
		let Some(file) = files.into_iter().find(|file| {
			file.file_id == file_id
				&& file.path == path
				&& matches!(
					file.scope,
					crate::apps::execution::capabilities::contracts::FileScope::Working
						| crate::apps::execution::capabilities::contracts::FileScope::Received
				)
		}) else {
			return Ok(None);
		};
		// `read` resolves by File ID, so never let another Area shadow a
		// different version already in this batch.
		if self
			.files
			.iter()
			.any(|known| known.file_id == file.file_id && known.digest != file.digest)
		{
			return Err(aidash_application::Error::Conflict("FILE_CHANGED".into()));
		}
		let selected = SelectedFile {
			file_id: file.file_id,
			path: file.path.clone(),
			digest: file.digest.clone(),
			size: file.size,
			media_type: file.media_type.clone(),
		};
		self.files.push(file);
		Ok(Some(selected))
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

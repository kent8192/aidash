//! Native attachment reads retain position/id order and the original transaction.
use crate::{Result as NativeResult, apps::identity::services::access::Access, store::Store};
use aidash_application::{
	Result,
	ports::execution::media::{
		AuthorizedMediaScope, MediaAttachment, MediaAttachments, MediaTransaction,
		OperatorMediaRepository,
	},
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
pub(crate) struct OperatorMedia<'a> {
	pub(crate) store: &'a Store,
}
struct Transaction {
	tx: crate::database::native::Transaction,
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
#[async_trait]
impl OperatorMediaRepository for OperatorMedia<'_> {
	fn node_id(&self) -> &str {
		&self.store.node_id
	}
	async fn begin(&self) -> Result<Box<dyn MediaTransaction>> {
		let tx = crate::database::native::begin(&self.store.pool).await?;
		Ok(Box::new(Transaction { tx }))
	}
}
#[async_trait]
impl MediaAttachments for Transaction {
	async fn attachments(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<MediaAttachment>> {
		Attachments { tx: &mut self.tx }
			.attachments(workspace, message)
			.await
	}
}
#[async_trait]
impl MediaTransaction for Transaction {
	async fn commit(self: Box<Self>) -> Result<()> {
		let transaction = *self;
		transaction.tx.commit().await.map_err(Into::into)
	}
}

use crate::apps::execution::capabilities::{contracts::FileEntry, service, sessions};
use aidash_domain::{Run, media::SelectedFile};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

pub(crate) struct SelectedMedia<'a> {
	pub(crate) store: &'a Store,
	pub(crate) run: &'a Run,
	pub(crate) access: Arc<Mutex<Access>>,
	lease: Option<OwnedMutexGuard<Access>>,
	files: Vec<FileEntry>,
}
impl<'a> SelectedMedia<'a> {
	pub(crate) fn new(store: &'a Store, run: &'a Run, access: Arc<Mutex<Access>>) -> Self {
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
			self.lease = Some(self.access.clone().lock_owned().await);
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
}

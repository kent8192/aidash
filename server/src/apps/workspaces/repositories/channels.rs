//! Channel persistence ports on the existing policy and credential lease.
use crate::{
	apps::workspaces::{
		access::Lease,
		models::{
			ChannelAttachment as AttachmentRecord, ChannelMessageContext as ContextRecord,
			Message as MessageRecord,
		},
	},
	store::Store,
};
use aidash_application::{Result, ports::channels::*, threads::channels::AttachmentUpload};
use aidash_domain::{Message, threads::channels::*};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, IntoIden, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) struct NativeChannels<'a> {
	pub(crate) store: &'a Store,
	pub(crate) lease: &'a mut Lease,
	pub(crate) federation: Option<&'a crate::federation::Federation>,
	pub(crate) run_id: Option<Uuid>,
	/// The Agent writing for `run_id`. Its Run holds the lease in the name of the
	/// person who started it, who is not the sender of what the Agent writes.
	pub(crate) agent: Option<String>,
}
#[derive(Debug)]
struct LinkRecord {
	id: Uuid,
	filename: String,
	media_type: String,
	size_bytes: i64,
	message_id: Option<Uuid>,
	description: Option<String>,
	index_state: Option<String>,
}
crate::native_record!(LinkRecord {
	id,
	filename,
	media_type,
	size_bytes,
	message_id,
	description,
	index_state
});

impl LinkRecord {
	fn public(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
			no_assigned_agent_can_read: false,
			description: self.description.clone(),
			index_state: self.index_state.clone(),
		}
	}
}

/// Where an image attachment stands in its Thread index, as a status word
/// only; the entry itself stays under the semantic index's own authority.
fn index_state() -> SimpleExpr {
	SimpleExpr::from(Expr::cust(format!(
		"(SELECT CASE se.state WHEN 'READY' THEN 'indexed' WHEN 'PENDING' THEN 'pending' \
		 WHEN 'UNSUPPORTED' THEN 'unsupported' ELSE 'failed' END \
		 FROM semantic_entries se WHERE se.thread_id = channel_attachments.thread_id \
		 AND {} AND NOT se.deleted)",
		crate::apps::knowledge::repositories::attachment_images::names_attachment(
			"se",
			"channel_attachments"
		)
	)))
}

struct HistoryRecord {
	message: MessageRecord,
}
impl crate::database::Record for HistoryRecord {
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		Ok(Self {
			message: MessageRecord::decode(row)?,
		})
	}
}
#[async_trait]
impl AttachmentObjects for NativeChannels<'_> {
	async fn put_object(&mut self, bytes: &[u8], sha256: &str) -> Result<Uuid> {
		let (id, actual) = self
			.store
			.capabilities
			.put(self.lease.access_mut(), None, "thread_attachment", bytes)
			.await?;
		if actual != sha256 {
			return Err(crate::Error::Conflict("OBJECT_INTEGRITY".into()).into());
		}
		Ok(id)
	}
	async fn read_object(&mut self, id: Uuid, sha256: &str, size: i64) -> Result<Vec<u8>> {
		super::media::read_attachment_object(self.store, self.lease.access_mut(), id, sha256, size)
			.await
			.map_err(Into::into)
	}
}
impl aidash_application::ports::authorization::visibility::RowOutcome for NativeChannels<'_> {
	fn begin_row(&mut self) {
		self.lease.access_mut().begin_row();
	}
	fn row_silence(&self) -> Option<String> {
		self.lease.row_silence()
	}
	fn take_row_silence(&mut self) -> Option<String> {
		self.lease.access_mut().take_row_silence()
	}
}
#[async_trait]
impl ChannelScope for NativeChannels<'_> {
	fn sender(&self) -> String {
		self.agent.clone().unwrap_or_else(|| self.lease.sender())
	}
	fn principal(&self) -> String {
		match self.run_id {
			Some(run) => {
				crate::registry::digest(&json!({"principal":self.lease.principal(), "run_id":run}))
			}
			None => self.lease.principal(),
		}
	}
	async fn message(&mut self, thread: Uuid, id: Uuid) -> Result<Message> {
		self.lease.message(thread, id).await.map_err(Into::into)
	}
	async fn visible(&mut self, message: &Message) -> Result<bool> {
		self.lease.visible(message).await.map_err(Into::into)
	}
	async fn append_message(
		&mut self,
		thread: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<Message> {
		if let Some(run) = self.run_id {
			let message = MessageRecord::append_in(
				self.lease.tx().as_mut(),
				thread,
				sender,
				content,
				key,
				Some(run),
			)
			.await?;
			self.store
				.record_output_in(self.lease.tx(), Some(run), thread, "message", message.id)
				.await?;
			return Ok(message);
		}
		self.store
			.message_in_with_attachments(self.lease.tx(), thread, sender, content, Some(key))
			.await
			.map_err(Into::into)
	}
	async fn record_context(
		&mut self,
		thread: Uuid,
		message: Uuid,
		digest: &str,
	) -> Result<MessageContext> {
		{
			let query_bind_1 = message;
			let query_bind_2 = thread;
			let query_bind_4 = digest;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("channel_message_context"))
					.columns([
						Alias::new("message_id"),
						Alias::new("thread_id"),
						Alias::new("attachment_digest"),
						Alias::new("tenant"),
					])
					.from_subquery(
						Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.expr(crate::database::scope::tenant_of(
								"threads",
								"id",
								query_bind_2,
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::column(Alias::new("message_id"))
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **self.lease.tx())
			.await?
		};
		let context: ContextRecord =
			{
				let query_bind_1 = thread;
				let query_bind_2 = message;
				crate::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("channel_message_context"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.lease.tx())
				.await?
			};
		Ok(MessageContext {
			attachment_digest: context.attachment_digest,
		})
	}
	async fn legacy_attachment_positions(
		&mut self,
		thread: Uuid,
		message: Uuid,
	) -> Result<Vec<(Uuid, i32)>> {
		let rows: Vec<(Uuid, i32)> =
			{
				let query_bind_1 = thread;
				let query_bind_2 = message;
				crate::database::native::query_as(
					&Query::select()
						.columns([Alias::new("id"), Alias::new("position")])
						.from(Alias::new("channel_attachments"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["id", "position"])
				.fetch_all(&mut **self.lease.tx())
				.await?
			};
		Ok(rows)
	}
	async fn attachment_links(
		&mut self,
		thread: Uuid,
		ids: &[Uuid],
		principal: &str,
	) -> Result<Vec<AttachmentLink>> {
		let uploader = principal;
		let mut query = Query::select();
		query
			.columns([
				Alias::new("id"),
				Alias::new("filename"),
				Alias::new("media_type"),
				Alias::new("size_bytes"),
				Alias::new("message_id"),
				Alias::new("position"),
				Alias::new("description"),
			])
			.expr_as(Expr::cust("NULL::text"), Alias::new("index_state"))
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
					.eq(Expr::cust("$1")),
			)
			.and_where(Expr::cust("id = ANY($2)"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("uploaded_by")))
					.eq(Expr::cust("$3")),
			)
			.order_by(Alias::new("id"), Order::Asc)
			.lock(LockType::Update);
		let sql = query.to_string(PostgresQueryBuilder);
		let records: Vec<LinkRecord> = crate::database::native::query_as(&sql)
			.bind(thread)
			.bind(ids.to_vec())
			.bind(uploader)
			.fetch_all(&mut **self.lease.tx())
			.await?;
		Ok(records
			.into_iter()
			.map(|r| AttachmentLink {
				attachment: r.public(),
				message_id: r.message_id,
			})
			.collect())
	}
	async fn bind_attachment(
		&mut self,
		thread: Uuid,
		message: Uuid,
		id: Uuid,
		principal: &str,
		position: i32,
	) -> Result<()> {
		let uploader = principal;
		let update = Query::update()
			.table(Alias::new("channel_attachments"))
			.value_expr(Alias::new("message_id"), Expr::cust("$1"))
			.value_expr(Alias::new("position"), Expr::cust("$5"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
					.eq(Expr::cust("$2")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
					.eq(Expr::cust("$3")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("uploaded_by")))
					.eq(Expr::cust("$4")),
			)
			.and_where(Expr::col(Alias::new("message_id")).is_null())
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&update)
			.bind(message)
			.bind(thread)
			.bind(id)
			.bind(uploader)
			.bind(position)
			.execute(&mut **self.lease.tx())
			.await?;
		Ok(())
	}
	async fn insert_attachment(
		&mut self,
		thread: Uuid,
		principal: &str,
		input: &AttachmentUpload<'_>,
		digest: &str,
		content: &[u8],
	) -> Result<Option<AttachmentState>> {
		let uploader = principal;
		// Serialize replays before reserving quota for an object that may already exist.
		let lock_key = format!(
			"thread-attachment:{thread}:{uploader}:{}",
			input.idempotency_key
		);
		let lock = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![Expr::value(lock_key).into(), Expr::value(0_i64).into()],
				)],
			))
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&lock)
			.execute(&mut **self.lease.tx())
			.await?;
		let query = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("uploaded_by")).eq(Expr::value(uploader)))
			.and_where(
				Expr::col(Alias::new("idempotency_key")).eq(Expr::value(input.idempotency_key)),
			)
			.to_string(PostgresQueryBuilder);
		let existing: Option<Uuid> = crate::database::native::query_scalar(&query)
			.scalar_optional(&mut **self.lease.tx())
			.await?;
		if existing.is_some() {
			return Ok(None);
		}
		let size_bytes = i64::try_from(content.len())
			.map_err(|_| crate::Error::Invalid("attachment is too large".into()))?;
		let object_id = self.put_object(content, digest).await?;
		let tenant = Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("threads"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(thread)))
			.to_owned();
		let insert = Query::insert()
			.into_table(Alias::new("channel_attachments"))
			.columns([
				Alias::new("id"),
				Alias::new("thread_id"),
				Alias::new("uploaded_by"),
				Alias::new("idempotency_key"),
				Alias::new("filename"),
				Alias::new("media_type"),
				Alias::new("sha256"),
				Alias::new("size_bytes"),
				Alias::new("object_id"),
				Alias::new("width"),
				Alias::new("height"),
				Alias::new("duration_ms"),
				Alias::new("description"),
				Alias::new("tenant"),
			])
			.values_expr(vec![
				Expr::value(Uuid::new_v4()).into(),
				Expr::value(thread).into(),
				Expr::value(uploader.to_owned()).into(),
				Expr::value(input.idempotency_key).into(),
				Expr::value(input.filename.to_owned()).into(),
				Expr::value(input.media_type.to_owned()).into(),
				Expr::value(digest.to_owned()).into(),
				Expr::value(size_bytes).into(),
				Expr::value(object_id).into(),
				Expr::value(input.measurements.width).into(),
				Expr::value(input.measurements.height).into(),
				Expr::value(input.measurements.duration_ms).into(),
				Expr::value(input.description.map(str::to_owned)).into(),
				Expr::subquery(tenant).into(),
			])
			.expect("one value per column")
			.on_conflict(
				OnConflict::columns([
					Alias::new("thread_id"),
					Alias::new("uploaded_by"),
					Alias::new("idempotency_key"),
				])
				.do_nothing()
				.to_owned(),
			)
			.returning_all()
			.to_string(PostgresQueryBuilder);
		let inserted: Option<AttachmentRecord> = crate::database::query_as(&insert)
			.fetch_optional(&mut **self.lease.tx())
			.await?;
		if inserted.is_none() {
			let tenant = self.lease.access_mut().identity.tenant.clone();
			self.store
				.capabilities
				.erase_committed(self.lease.tx(), &tenant, object_id, false)
				.await?;
		}
		Ok(inserted.map(Into::into))
	}
	async fn existing_attachment(
		&mut self,
		thread: Uuid,
		principal: &str,
		key: Uuid,
	) -> Result<AttachmentState> {
		let uploader = principal;
		let select = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
					.eq(Expr::cust("$1")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("uploaded_by")))
					.eq(Expr::cust("$2")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("idempotency_key")))
					.eq(Expr::cust("$3")),
			)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder);
		let existing: AttachmentRecord = crate::database::query_as(&select)
			.bind(thread)
			.bind(uploader)
			.bind(key)
			.fetch_one(&mut **self.lease.tx())
			.await?;
		Ok(existing.into())
	}
	async fn attachment(&mut self, thread: Uuid, id: Uuid) -> Result<Option<AttachmentState>> {
		let query = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
					.eq(Expr::cust("$1")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
					.eq(Expr::cust("$2")),
			)
			.to_string(PostgresQueryBuilder);
		let attachment: Option<AttachmentRecord> = crate::database::query_as(&query)
			.bind(thread)
			.bind(id)
			.fetch_optional(&mut **self.lease.tx())
			.await?;
		Ok(attachment.map(Into::into))
	}
	async fn history_rows(
		&mut self,
		thread: Uuid,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<HistoryRow>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::table_asterisk(Alias::new("m")))
			.from_as(Alias::new("messages"), Alias::new("m"))
			.and_where(
				Expr::col((Alias::new("m"), Alias::new("thread_id"))).eq(Expr::value(thread)),
			)
			.order_by((Alias::new("m"), Alias::new("created_at")), Order::Desc)
			.order_by((Alias::new("m"), Alias::new("id")), Order::Desc)
			.limit(100);
		if let Some((created_at, id)) = cursor {
			query.and_where(
				Condition::any()
					.add(
						Expr::col((Alias::new("m"), Alias::new("created_at")))
							.lt(Expr::value(created_at)),
					)
					.add(
						Condition::all()
							.add(
								Expr::col((Alias::new("m"), Alias::new("created_at")))
									.eq(Expr::value(created_at)),
							)
							.add(
								Expr::col((Alias::new("m"), Alias::new("id"))).lt(Expr::value(id)),
							),
					),
			);
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let rows = aidash_server::database::query_as::<HistoryRecord>(&sql)
			.fetch_all(&mut **self.lease.tx())
			.await?;
		Ok(rows
			.into_iter()
			.map(|r| HistoryRow {
				message: r.message.into(),
			})
			.collect())
	}
	async fn message_attachments(
		&mut self,
		thread: Uuid,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, Vec<ChannelAttachment>>> {
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
				Alias::new("position"),
				Alias::new("description"),
			])
			.expr_as(index_state(), Alias::new("index_state"))
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
					.eq(Expr::cust("$1")),
			)
			.and_where(Expr::cust("message_id = ANY($2)"))
			.order_by(Alias::new("position"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.to_string(PostgresQueryBuilder);
		let records: Vec<LinkRecord> = crate::database::native::query_as(&query)
			.columns(&[
				"id",
				"filename",
				"media_type",
				"size_bytes",
				"message_id",
				"position",
				"description",
				"index_state",
			])
			.bind(thread)
			.bind(ids.to_vec())
			.fetch_all(&mut **self.lease.tx())
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
	async fn record_citations(
		&mut self,
		_: Uuid,
		message: Uuid,
		citations: &[aidash_domain::threads::channels::CitedFileVersion],
	) -> Result<Vec<aidash_domain::threads::channels::ChannelCitation>> {
		super::citations::require_supported(citations)?;
		let mut stored = super::citations::for_messages(
			self.store,
			self.federation,
			&mut *self.lease,
			&[message],
		)
		.await?;
		Ok(stored.remove(&message).unwrap_or_default())
	}
	async fn message_citations(
		&mut self,
		_: Uuid,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, Vec<aidash_domain::threads::channels::ChannelCitation>>> {
		super::citations::for_messages(self.store, self.federation, &mut *self.lease, ids)
			.await
			.map_err(Into::into)
	}
	async fn describe_attachment(
		&mut self,
		thread: Uuid,
		id: Uuid,
		writer: &str,
		description: &str,
	) -> Result<()> {
		let update = Query::update()
			.table(Alias::new("channel_attachments"))
			.value(Alias::new("description"), description)
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.and_where(Expr::col(Alias::new("uploaded_by")).eq(Expr::value(writer)))
			.to_string(PostgresQueryBuilder);
		let updated = crate::database::native::query(&update)
			.execute(&mut **self.lease.tx())
			.await?;
		if updated.rows_affected() != 1 {
			return Err(aidash_application::Error::Forbidden);
		}
		// The words are embedded with the image, so its entry is queued again.
		let mut lease =
			crate::apps::knowledge::repositories::access::Lease::Inherited(self.lease.access_mut());
		crate::apps::knowledge::repositories::attachment_images::enqueue(
			&mut lease,
			thread,
			&[id],
			true,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn index_attachment_images(
		&mut self,
		thread: Uuid,
		_message: Uuid,
		attachments: &[ChannelAttachment],
	) -> Result<()> {
		let ids: Vec<Uuid> = attachments
			.iter()
			.filter(|attachment| attachment.media_type.starts_with("image/"))
			.map(|attachment| attachment.id)
			.collect();
		let mut lease =
			crate::apps::knowledge::repositories::access::Lease::Inherited(self.lease.access_mut());
		crate::apps::knowledge::repositories::attachment_images::enqueue(
			&mut lease, thread, &ids, false,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
}

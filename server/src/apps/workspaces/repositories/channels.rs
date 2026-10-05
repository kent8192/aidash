//! Channel persistence ports on the existing policy and credential lease.
use crate::{
	apps::workspaces::{
		access::Lease,
		models::{
			ChannelAttachment as AttachmentRecord, ChannelMessageContext as ContextRecord,
			ChannelThread as ThreadRecord, Message as MessageRecord,
		},
	},
	store::Store,
};
use aidash_application::{Result, ports::channels::*, workspaces::channels::AttachmentUpload};
use aidash_domain::{Message, workspaces::channels::*};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, JoinType, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr, TableRef,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) struct NativeChannels<'a> {
	pub(crate) store: &'a Store,
	pub(crate) lease: &'a mut Lease,
}
#[derive(Debug)]
struct LinkRecord {
	id: Uuid,
	filename: String,
	media_type: String,
	size_bytes: i64,
	message_id: Option<Uuid>,
}
crate::native_record!(LinkRecord {
	id,
	filename,
	media_type,
	size_bytes,
	message_id
});

impl LinkRecord {
	fn public(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
		}
	}
}

struct HistoryRecord {
	message: MessageRecord,
	reply_thread_id: Option<Uuid>,
	root_thread_id: Option<Uuid>,
}
impl crate::database::Record for HistoryRecord {
	fn decode(row: &crate::database::native::Row) -> crate::Result<Self> {
		Ok(Self {
			message: MessageRecord::decode(row)?,
			reply_thread_id: row.try_get("reply_thread_id")?,
			root_thread_id: row.try_get("root_thread_id")?,
		})
	}
}
#[async_trait]
impl ChannelScope for NativeChannels<'_> {
	fn sender(&self) -> String {
		self.lease.sender()
	}
	fn principal(&self) -> String {
		self.lease.principal()
	}
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<Message> {
		self.lease.message(workspace, id).await.map_err(Into::into)
	}
	async fn visible(&mut self, message: &Message) -> Result<bool> {
		self.lease.visible(message).await.map_err(Into::into)
	}
	async fn thread_visible(&mut self, id: Uuid) -> Result<()> {
		crate::capabilities::thread_lifecycle::visible(self.lease.tx(), id)
			.await
			.map_err(Into::into)
	}
	async fn thread(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<ChannelThread>> {
		let thread: Option<ThreadRecord> = {
			let query_bind_1 = workspace;
			let query_bind_2 = id;
			crate::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("channel_threads"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.lease.tx())
			.await?
		};
		Ok(thread.map(Into::into))
	}
	async fn reply_thread(&mut self, message: Uuid) -> Result<Option<Uuid>> {
		let thread: Option<Option<Uuid>> = {
			let query_bind_1 = message;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("thread_id"))
					.from(Alias::new("channel_message_context"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.lease.tx())
			.await?
		};
		Ok(thread.flatten())
	}
	async fn insert_thread(
		&mut self,
		workspace: Uuid,
		root: Uuid,
		sender: &str,
	) -> Result<Option<ChannelThread>> {
		let inserted: Option<ThreadRecord> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace;
			let query_bind_3 = root;
			let query_bind_4 = sender;
			crate::database::query_as(
				&Query::insert()
					.into_table(Alias::new("channel_threads"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("root_message_id"),
						Alias::new("created_by"),
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
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::column(Alias::new("root_message_id"))
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.lease.tx())
			.await?
		};
		Ok(inserted.map(Into::into))
	}
	async fn existing_thread(&mut self, workspace: Uuid, root: Uuid) -> Result<ChannelThread> {
		let existing: ThreadRecord = {
			let query_bind_1 = workspace;
			let query_bind_2 = root;
			crate::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("channel_threads"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
							"root_message_id",
						)))
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
		Ok(existing.into())
	}
	async fn thread_event(&mut self, workspace: Uuid, root: Uuid, thread: Uuid) -> Result<()> {
		self.store
			.event(
				self.lease.tx(),
				Some(workspace),
				"message.thread_opened",
				json!({"id":root,"thread_id":thread}),
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn append_message(
		&mut self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<Message> {
		self.store
			.message_in_with_attachments(self.lease.tx(), workspace, sender, content, Some(key))
			.await
			.map_err(Into::into)
	}
	async fn record_context(
		&mut self,
		workspace: Uuid,
		message: Uuid,
		thread: Option<Uuid>,
		digest: &str,
	) -> Result<MessageContext> {
		{
			let query_bind_1 = message;
			let query_bind_2 = workspace;
			let query_bind_3 = thread;
			let query_bind_4 = digest;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("channel_message_context"))
					.columns([
						Alias::new("message_id"),
						Alias::new("workspace_id"),
						Alias::new("thread_id"),
						Alias::new("attachment_digest"),
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
								vec![Expr::value(query_bind_3.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
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
		let context: ContextRecord = {
			let query_bind_1 = workspace;
			let query_bind_2 = message;
			crate::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("channel_message_context"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **self.lease.tx())
			.await?
		};
		Ok(MessageContext {
			thread_id: context.thread_key,
			attachment_digest: context.attachment_digest,
		})
	}
	async fn root_thread(&mut self, message: Uuid) -> Result<Option<Uuid>> {
		let root_thread: Option<Uuid> = {
			let query_bind_1 = message;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("channel_threads"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
							"root_message_id",
						)))
						.eq(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.lease.tx())
			.await?
		};
		Ok(root_thread)
	}
	async fn legacy_attachment_positions(
		&mut self,
		workspace: Uuid,
		message: Uuid,
	) -> Result<Vec<(Uuid, i32)>> {
		let rows: Vec<(Uuid, i32)> = {
			let query_bind_1 = workspace;
			let query_bind_2 = message;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("id"), Alias::new("position")])
					.from(Alias::new("channel_attachments"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("message_id"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
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
		workspace: Uuid,
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
			])
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
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
			.bind(workspace)
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
		workspace: Uuid,
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
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
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
			.bind(workspace)
			.bind(id)
			.bind(uploader)
			.bind(position)
			.execute(&mut **self.lease.tx())
			.await?;
		Ok(())
	}
	async fn insert_attachment(
		&mut self,
		workspace: Uuid,
		principal: &str,
		input: &AttachmentUpload<'_>,
		digest: &str,
		content: &[u8],
	) -> Result<Option<AttachmentState>> {
		let uploader = principal;
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
			.from_subquery(
				Query::select()
					.expr(Expr::cust("$1"))
					.expr(Expr::cust("$2"))
					.expr(Expr::cust("$3"))
					.expr(Expr::cust("$4"))
					.expr(Expr::cust("$5"))
					.expr(Expr::cust("$6"))
					.expr(Expr::cust("$7"))
					.expr(Expr::cust("$8"))
					.expr(Expr::cust("$9"))
					.to_owned(),
			)
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
		let inserted: Option<AttachmentRecord> = crate::database::query_as(&insert)
			.bind(Uuid::new_v4())
			.bind(workspace)
			.bind(uploader)
			.bind(input.idempotency_key)
			.bind(input.filename)
			.bind(input.media_type)
			.bind(digest)
			.bind(
				i64::try_from(content.len())
					.map_err(|_| crate::Error::Invalid("attachment is too large".into()))?,
			)
			.bind(content)
			.fetch_optional(&mut **self.lease.tx())
			.await?;
		Ok(inserted.map(Into::into))
	}
	async fn existing_attachment(
		&mut self,
		workspace: Uuid,
		principal: &str,
		key: Uuid,
	) -> Result<AttachmentState> {
		let uploader = principal;
		let select = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
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
			.bind(workspace)
			.bind(uploader)
			.bind(key)
			.fetch_one(&mut **self.lease.tx())
			.await?;
		Ok(existing.into())
	}
	async fn attachment(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<AttachmentState>> {
		let query = Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
					.eq(Expr::cust("$1")),
			)
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
					.eq(Expr::cust("$2")),
			)
			.to_string(PostgresQueryBuilder);
		let attachment: Option<AttachmentRecord> = crate::database::query_as(&query)
			.bind(workspace)
			.bind(id)
			.fetch_optional(&mut **self.lease.tx())
			.await?;
		Ok(attachment.map(Into::into))
	}
	async fn history_rows(
		&mut self,
		workspace: Uuid,
		thread: Option<&ChannelThread>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<HistoryRow>> {
		let mut query = Query::select();
		query
			.column(ColumnRef::table_asterisk(Alias::new("m")))
			.expr_as(
				Expr::col((Alias::new("c"), Alias::new("thread_id"))),
				Alias::new("reply_thread_id"),
			)
			.expr_as(
				Expr::col((Alias::new("t"), Alias::new("id"))),
				Alias::new("root_thread_id"),
			)
			.from_as(Alias::new("messages"), Alias::new("m"))
			.and_where(
				Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from_as(Alias::new("core_records"), Alias::new("d"))
						.and_where(
							Expr::col((Alias::new("d"), Alias::new("kind")))
								.eq(reinhardt::query::Expr::value("thread_tombstone")),
						)
						.and_where(Expr::cust("d.data->>'root_message_id' = m.id::text"))
						.to_owned(),
				)
				.not(),
			)
			.join(
				JoinType::LeftJoin,
				TableRef::table_alias(Alias::new("channel_message_context"), Alias::new("c")),
				Expr::col((Alias::new("c"), Alias::new("message_id")))
					.equals((Alias::new("m"), Alias::new("id"))),
			)
			.join(
				JoinType::LeftJoin,
				TableRef::table_alias(Alias::new("channel_threads"), Alias::new("t")),
				Expr::col((Alias::new("t"), Alias::new("root_message_id")))
					.equals((Alias::new("m"), Alias::new("id"))),
			)
			.and_where(
				Expr::col((Alias::new("m"), Alias::new("workspace_id"))).eq(Expr::value(workspace)),
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
		if let Some(thread) = thread {
			query.and_where(
				Condition::any()
					.add(
						Expr::col((Alias::new("c"), Alias::new("thread_id")))
							.eq(Expr::value(thread.id)),
					)
					.add(
						Expr::col((Alias::new("m"), Alias::new("id")))
							.eq(Expr::value(thread.root_message_id)),
					),
			);
		} else {
			query.and_where(Expr::col((Alias::new("c"), Alias::new("thread_id"))).is_null());
		}
		let sql = query.to_string(PostgresQueryBuilder);
		let rows = aidash_server::database::query_as::<HistoryRecord>(&sql)
			.fetch_all(&mut **self.lease.tx())
			.await?;
		Ok(rows
			.into_iter()
			.map(|r| HistoryRow {
				message: r.message.into(),
				reply_thread_id: r.reply_thread_id,
				root_thread_id: r.root_thread_id,
			})
			.collect())
	}
	async fn message_attachments(
		&mut self,
		workspace: Uuid,
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
			])
			.from(Alias::new("channel_attachments"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
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
			])
			.bind(workspace)
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
}

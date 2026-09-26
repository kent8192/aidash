use super::{ChannelMessage, ChannelMessageInput, ChannelThread, access::Lease, attachments};
use crate::{Error, Result, domain::nonempty, store::Store};
use sea_orm::sea_query::{Alias, Asterisk, Expr, OnConflict, PostgresQueryBuilder, Query};
use serde_json::json;
use uuid::Uuid;

pub(crate) async fn get(lease: &mut Lease, workspace: Uuid, id: Uuid) -> Result<ChannelThread> {
	crate::capabilities::thread_lifecycle::visible(lease.tx(), id).await?;
	let thread: Option<ChannelThread> = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("channel_threads"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(id)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let thread = thread.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
	lease.message(workspace, thread.root_message_id).await?;
	Ok(thread)
}

pub(crate) async fn create(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	root: Uuid,
) -> Result<ChannelThread> {
	lease.message(workspace, root).await?;
	if reply_thread(lease, root).await?.is_some() {
		return Err(Error::Conflict(
			"a reply already belongs to a thread".into(),
		));
	}
	let sender = lease.sender();
	let inserted: Option<ChannelThread> = sqlx::query_as(
		&Query::insert()
			.into_table(Alias::new("channel_threads"))
			.columns([
				Alias::new("id"),
				Alias::new("workspace_id"),
				Alias::new("root_message_id"),
				Alias::new("created_by"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
			])
			.on_conflict(
				OnConflict::column(Alias::new("root_message_id"))
					.do_nothing()
					.to_owned(),
			)
			.returning_all()
			.to_string(PostgresQueryBuilder),
	)
	.bind(Uuid::new_v4())
	.bind(workspace)
	.bind(root)
	.bind(sender)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if let Some(thread) = inserted {
		store
			.event(
				lease.tx(),
				Some(workspace),
				"message.thread_opened",
				json!({"id":root,"thread_id":thread.id}),
			)
			.await?;
		return Ok(thread);
	}
	let existing: ChannelThread = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("channel_threads"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("root_message_id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(root)
	.fetch_one(&mut **lease.tx())
	.await?;
	crate::capabilities::thread_lifecycle::visible(lease.tx(), existing.id).await?;
	Ok(existing)
}

pub(crate) async fn reply_thread(lease: &mut Lease, message: Uuid) -> Result<Option<Uuid>> {
	let thread: Option<Option<Uuid>> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("thread_id"))
			.from(Alias::new("channel_message_context"))
			.and_where(Expr::col(Alias::new("message_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(message)
	.fetch_optional(&mut **lease.tx())
	.await?;
	Ok(thread.flatten())
}

pub(crate) async fn post(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelMessageInput,
) -> Result<ChannelMessage> {
	nonempty(&input.content, "message")?;
	let attachment_digest = attachments::digest_ids(&input.attachment_ids)?;
	if let Some(thread) = input.thread_id {
		get(lease, workspace, thread).await?;
	}
	let sender = lease.sender();
	let key = format!(
		"channel:{workspace}:{}:{}",
		lease.principal(),
		input.idempotency_key
	);
	let message = store
		.message_in(lease.tx(), workspace, &sender, &input.content, Some(&key))
		.await?;
	// Root submissions also have a context row, binding their idempotency key
	// to the absence of a thread rather than allowing a later reply replay.
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("channel_message_context"))
			.columns([
				Alias::new("message_id"),
				Alias::new("workspace_id"),
				Alias::new("thread_id"),
				Alias::new("attachment_digest"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
			])
			.on_conflict(
				OnConflict::column(Alias::new("message_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(message.id)
	.bind(workspace)
	.bind(input.thread_id)
	.bind(&attachment_digest)
	.execute(&mut **lease.tx())
	.await?;
	#[derive(sqlx::FromRow)]
	struct StoredContext {
		thread_id: Option<Uuid>,
		attachment_digest: String,
	}
	let context: StoredContext = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("thread_id"), Alias::new("attachment_digest")])
			.from(Alias::new("channel_message_context"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("message_id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(message.id)
	.fetch_one(&mut **lease.tx())
	.await?;
	let stored_thread = context.thread_id;
	if stored_thread != input.thread_id {
		return Err(Error::Conflict(
			"message idempotency key reused for a different thread".into(),
		));
	}
	if context.attachment_digest != attachment_digest {
		return Err(Error::Conflict(
			"message idempotency key reused for different attachments".into(),
		));
	}
	let message_attachments =
		attachments::attach(lease, workspace, message.id, &input.attachment_ids).await?;
	let root_thread: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("channel_threads"))
			.and_where(Expr::col(Alias::new("root_message_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(message.id)
	.fetch_optional(&mut **lease.tx())
	.await?;
	Ok(ChannelMessage {
		message,
		thread_id: stored_thread.or(root_thread),
		is_thread_root: root_thread.is_some(),
		attachments: message_attachments,
	})
}

//! Record reads keep thread predicates and strict status decoding in one transaction.
use super::Reads;
use crate::Result as NativeResult;
use crate::apps::workspaces::models::child_flags;
use aidash_application::{
	Result,
	ports::authorization::records::{ChildSummaryScope, ChildTaskRecord, ThreadRecordScope},
};
use aidash_domain::{Event, Message, Task, Thread, ThreadSnapshot, policy::Resource};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
struct AttachmentRecordRow {
	message_id: Uuid,
	id: Uuid,
	filename: String,
	media_type: String,
	size_bytes: i64,
	width: Option<i32>,
	height: Option<i32>,
	duration_ms: Option<i64>,
	sha256: String,
	description: Option<String>,
}
crate::native_record!(AttachmentRecordRow {
	message_id,
	id,
	filename,
	media_type,
	size_bytes,
	width,
	height,
	duration_ms,
	sha256,
	description
});
const CREATOR_THREAD_BOUNDARY: &str = "That record is in the Thread of the Agent that created this Subnode, which members cannot read. Ask the creator to put what you need in the Task or the Subnode Workspace. Do not repeat this call.";

pub(crate) fn wrong_thread(kind: &str, id: Uuid) -> crate::Error {
	crate::Error::Invalid(format!(
		"{kind} {id} is not in the Thread this call reads. To read a Subnode this Run created, pass its thread_id. Do not repeat this call unchanged."
	))
}

/// Describe the immediate creator boundary using only the existence of the
/// supplied ID in that Thread. Unrelated Threads are never queried.
pub(crate) async fn in_creator_thread(
	access: &mut super::Access,
	member_thread: Uuid,
	kind: &str,
	id: Uuid,
) -> NativeResult<bool> {
	use crate::apps::execution::models::subnodes::SubnodeRecord;
	let table = match kind {
		"thread" => "threads",
		"task" => "tasks",
		"message" => "messages",
		"event" => "events",
		"attachment" => "channel_attachments",
		_ => return Ok(false),
	};
	let tenant = access.identity.tenant.clone();
	let Some(creator) =
		SubnodeRecord::creator_of_thread(&mut *access.tx, &tenant, member_thread).await?
	else {
		return Ok(false);
	};
	let creator_thread = Query::select()
		.column(Alias::new("thread_id"))
		.from(Alias::new("task_executions"))
		.and_where(Expr::col("id").eq(Expr::value(creator)))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.to_owned();
	let thread_column = if kind == "thread" { "id" } else { "thread_id" };
	let mut record = Query::select();
	record
		.expr(Expr::value(true))
		.from(Alias::new(table))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col(thread_column).in_subquery(creator_thread));
	if kind == "attachment" {
		record.and_where(Expr::col("message_id").is_not_null());
	}
	Ok(
		crate::database::native::query_scalar(&record.to_string(PostgresQueryBuilder))
			.scalar_optional(&mut *access.tx)
			.await?
			.unwrap_or(false),
	)
}

pub(crate) fn creator_thread_boundary() -> crate::Error {
	crate::Error::Invalid(CREATOR_THREAD_BOUNDARY.into())
}

#[async_trait]
impl ThreadRecordScope for Reads<'_> {
	async fn thread(&mut self, id: Uuid) -> Result<Resource> {
		Box::pin(self.access.thread(id)).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		Box::pin(self.access.require(resource, action))
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		Box::pin(self.access.decide(resource, action))
			.await
			.map_err(Into::into)
	}
	async fn task_visible(&mut self, row: &Task) -> Result<bool> {
		Box::pin(self.access.task_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn message_attachments(
		&mut self,
		thread: Uuid,
		messages: &[Uuid],
	) -> Result<Vec<aidash_domain::media::MessageAttachment>> {
		super::attachment_metadata(self.access, thread, messages).await
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		Box::pin(self.access.message_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn event_visible(&mut self, row: &Event) -> Result<bool> {
		Box::pin(self.access.event_visible(row))
			.await
			.map_err(Into::into)
	}
	async fn track(&mut self, snapshot: &ThreadSnapshot) -> Result<()> {
		aidash_application::authorization::journals::track_snapshot(self, snapshot).await
	}
	async fn missing_record(&mut self, kind: &str, id: Uuid) -> Result<aidash_application::Error> {
		let Some(member_thread) = self.tool_thread else {
			return Ok(aidash_application::Error::Forbidden);
		};
		let error = if in_creator_thread(self.access, member_thread, kind, id).await? {
			creator_thread_boundary()
		} else {
			wrong_thread(kind, id)
		};
		Ok(error.into())
	}
	async fn thread_row(&mut self, thread_id: Uuid) -> Result<Thread> {
		let result: NativeResult<Thread> = async {
			Ok({
				let query_bind_1 = thread_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("threads"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn task_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Task>> {
		let result: NativeResult<Option<Task>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = thread_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("tasks"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"thread_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn attachment_record(
		&mut self,
		thread: Uuid,
		id: Uuid,
	) -> Result<Option<aidash_domain::media::AttachmentRecord>> {
		let query = Query::select()
			.columns(
				[
					"message_id",
					"id",
					"filename",
					"media_type",
					"size_bytes",
					"width",
					"height",
					"duration_ms",
					"sha256",
					"description",
				]
				.map(Alias::new),
			)
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::value(thread)))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.and_where(Expr::col(Alias::new("message_id")).is_not_null())
			.to_string(PostgresQueryBuilder);
		let row: Option<AttachmentRecordRow> = crate::database::native::query_as(&query)
			.columns(&[
				"message_id",
				"id",
				"filename",
				"media_type",
				"size_bytes",
				"width",
				"height",
				"duration_ms",
				"sha256",
				"description",
			])
			.fetch_optional(&mut **self.access.tx)
			.await?;
		Ok(row.map(|row| aidash_domain::media::AttachmentRecord {
			message_id: row.message_id,
			sha256: row.sha256,
			attachment: aidash_domain::media::ThreadAttachment {
				id: row.id,
				filename: row.filename,
				media_type: row.media_type,
				size_bytes: row.size_bytes,
				width: row.width,
				height: row.height,
				duration_ms: row.duration_ms,
				description: row.description,
			},
		}))
	}
	async fn message_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Message>> {
		if let Some(message) = self.access.state_source_message(id, thread_id) {
			return Ok(message);
		}
		if let Some(message) = self.access.read_lookups.message(id, thread_id) {
			return Ok(message);
		}
		let result: NativeResult<Option<Message>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = thread_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("messages"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"thread_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event_record(&mut self, thread_id: Uuid, id: Uuid) -> Result<Option<Event>> {
		let result: NativeResult<Option<Event>> = async {
			Ok({
				let query_bind_1 = id;
				let query_bind_2 = thread_id;
				aidash_server::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("events"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"thread_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
struct ChildTaskSummaryRow {
	revision: i64,
	id: Uuid,
	title: String,
	created_by: String,
	status: crate::domain::TaskStatus,
	dependencies: Vec<Uuid>,
	started: bool,
	abandonable: bool,
}
crate::native_record!(ChildTaskSummaryRow {
	revision,
	id,
	title,
	created_by,
	status,
	dependencies,
	started,
	abandonable
});

#[async_trait]
impl ChildSummaryScope for Reads<'_> {
	async fn thread(&mut self, id: Uuid) -> Result<Resource> {
		self.access.thread(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn visible(
		&mut self,
		thread: &Resource,
		thread_id: Uuid,
		id: Uuid,
		created_by: &str,
	) -> Result<bool> {
		self.access
			.task_summary_visible(thread, thread_id, id, created_by)
			.await
			.map_err(Into::into)
	}
	async fn track_tasks(&mut self, thread: Uuid, tasks: &[Uuid]) -> Result<()> {
		aidash_application::authorization::journals::track_tasks(self, thread, tasks).await
	}
	async fn children(
		&mut self,
		thread_id: Uuid,
		parent_id: Uuid,
		after: Option<Uuid>,
	) -> Result<Vec<ChildTaskRecord>> {
		let result: NativeResult<Vec<ChildTaskRecord>> = async {
			let rows: Vec<ChildTaskSummaryRow> = {
				let query_bind_1 = thread_id;
				let query_bind_2 = parent_id;
				let query_bind_3 = after;
				crate::database::native::query_as(
					&Query::select()
						.columns([
							Alias::new("id"),
							Alias::new("title"),
							Alias::new("created_by"),
							Alias::new("status"),
							Alias::new("revision"),
							Alias::new("dependencies"),
						])
						.expr_as(
							child_flags::open_or_claimed().and(child_flags::started()),
							Alias::new("started"),
						)
						.expr_as(
							child_flags::open_or_claimed().and(child_flags::abandonable(parent_id)),
							Alias::new("abandonable"),
						)
						.from(Alias::new("tasks"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("parent_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							Condition::any()
								.add(
									SimpleExpr::CustomWithExpr(
										"(?::uuid)".to_owned(),
										vec![Expr::value(query_bind_3.to_owned()).into()],
									)
									.is_null(),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
										.gt(SimpleExpr::CustomWithExpr(
											"(?::uuid)".to_owned(),
											vec![Expr::value(query_bind_3.to_owned()).into()],
										)),
								),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(100)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&[
					"id",
					"title",
					"created_by",
					"status",
					"revision",
					"dependencies",
					"started",
					"abandonable",
				])
				.fetch_all(&mut **self.access.tx)
				.await?
			};
			Ok(rows
				.into_iter()
				.map(|row| ChildTaskRecord {
					revision: row.revision,
					id: row.id,
					title: row.title,
					created_by: row.created_by,
					status: row.status,
					dependencies: row.dependencies,
					started: row.started,
					abandonable: row.abandonable,
				})
				.collect())
		}
		.await;
		result.map_err(Into::into)
	}
}

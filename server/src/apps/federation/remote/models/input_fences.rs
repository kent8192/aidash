//! Home-node input reservations share the authoritative task row lock.
use super::RemoteRunMessageFence;
use crate::apps::federation::remote::services::input_fences::terminal;
use crate::apps::workspaces::models::{Message, Task};
use crate::apps::workspaces::serializers::entities::Message as MessageContract;
use crate::store::RunMessageDelivery;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::query::FieldAssignment;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

pub(crate) enum TerminalInputs<'a> {
	Keys(&'a [String]),
	Through(i64),
}

async fn locked_task(tx: &mut AtomicTransaction, id: Uuid) -> Result<Task> {
	Task::objects()
		.filter(Task::field_id().eq(id))
		.select_for_update()
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("task".into()))
}

async fn database_time(tx: &mut AtomicTransaction) -> Result<DateTime<Utc>> {
	let (sql, values) = Query::select()
		.expr_as(Expr::current_timestamp(), Alias::new("now"))
		.build(PostgresQueryBuilder);
	let row = TransactionExecutor::fetch_one(tx, &sql, convert_values(values)).await?;
	Ok(row.get("now").map_err(FrameworkError::from)?)
}

impl RemoteRunMessageFence {
	async fn by_key(tx: &mut AtomicTransaction, task: Uuid, key: &str) -> Result<Option<Self>> {
		Ok(Self::objects()
			.filter(Self::field_task_id().eq(task))
			.filter(Self::field_idempotency_key().eq(key))
			.all_with_db(tx)
			.await?
			.pop())
	}

	pub(crate) async fn reserve(
		tx: &mut AtomicTransaction,
		task_id: Uuid,
		run: Uuid,
		peer: &str,
		key: &str,
		content: &str,
	) -> Result<()> {
		let task = locked_task(tx, task_id).await?;
		let terminal = terminal(&task.status);
		let now = database_time(tx).await?;
		let delivered = Message::objects()
			.filter(Message::field_workspace_id().eq(task.workspace_id))
			.filter(Message::field_idempotency_key().eq(Some(format!("{peer}:{task_id}:{key}"))))
			.filter(Message::field_sender().eq(format!("human@{peer}")))
			.filter(Message::field_content().eq(content))
			.exists_with_db(tx)
			.await?;
		if let Some(previous) = Self::by_key(tx, task_id, key).await? {
			if previous.run_id != run || previous.content != content {
				return Err(Error::Conflict("run message idempotency key reused".into()));
			}
			if terminal {
				if previous.expires_at.is_none() || previous.consumed {
					return Ok(());
				}
				if !delivered {
					return Err(Error::Conflict("home task is terminal".into()));
				}
				Self::objects()
					.filter(Self::field_task_id().eq(previous.task_id))
					.filter(Self::field_idempotency_key().eq(&previous.idempotency_key))
					.update_fields_with_conn(
						tx,
						[(Self::field_expires_at(), None::<DateTime<Utc>>)],
					)
					.await?;
			} else if previous.expires_at.is_some_and(|deadline| deadline <= now) {
				Self::objects()
					.filter(Self::field_task_id().eq(previous.task_id))
					.filter(Self::field_idempotency_key().eq(&previous.idempotency_key))
					.update_fields_with_conn(
						tx,
						[(Self::field_expires_at(), Some(now + Duration::seconds(60)))],
					)
					.await?;
			}
		} else {
			if terminal && !delivered {
				return Err(Error::Conflict("home task is terminal".into()));
			}
			let record = Self::build()
				.task_id(task_id)
				.run_id(run)
				.idempotency_key(key.to_owned())
				.content(content.to_owned())
				.input_seq(None)
				.expires_at((!terminal).then_some(now + Duration::seconds(60)))
				.consumed(false)
				.finish();
			Self::objects().create_with_conn(tx, &record).await?;
		}
		Ok(())
	}

	pub(crate) async fn commit(
		tx: &mut AtomicTransaction,
		task_id: Uuid,
		run: Uuid,
		key: &str,
		content: &str,
		input_seq: Option<i64>,
	) -> Result<()> {
		if input_seq.is_some_and(|seq| seq <= 0) {
			return Err(Error::Invalid("invalid admitted input sequence".into()));
		}
		let task = locked_task(tx, task_id).await?;
		let now = database_time(tx).await?;
		let record = Self::by_key(tx, task_id, key)
			.await?
			.filter(|record| record.run_id == run)
			.ok_or_else(|| Error::Conflict("remote run message was not reserved at home".into()))?;
		if record.content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if input_seq
			.zip(record.input_seq)
			.is_some_and(|(next, previous)| next != previous)
		{
			return Err(Error::Conflict("run message input sequence changed".into()));
		}
		let committed = record.expires_at.is_none();
		if committed && (input_seq.is_none() || input_seq == record.input_seq) {
			return Ok(());
		}
		// An acknowledgement identifies an input already persisted by the delegated
		// executor. Its exact reservation remains recoverable after lease expiry.
		if !committed
			&& input_seq.is_none()
			&& (record.expires_at.is_some_and(|deadline| deadline <= now) || terminal(&task.status))
		{
			return Err(Error::Conflict(
				"remote run message reservation expired before admission was committed".into(),
			));
		}
		Self::objects()
			.filter(Self::field_task_id().eq(record.task_id))
			.filter(Self::field_idempotency_key().eq(&record.idempotency_key))
			.update_fields_with_conn::<_, _, FieldAssignment>(
				tx,
				vec![
					(Self::field_expires_at(), None::<DateTime<Utc>>).into(),
					(Self::field_input_seq(), record.input_seq.or(input_seq)).into(),
				],
			)
			.await?;
		Ok(())
	}

	pub(crate) async fn release(
		tx: &mut AtomicTransaction,
		task_id: Uuid,
		run: Uuid,
		peer: &str,
		keys: &[String],
	) -> Result<()> {
		let task = locked_task(tx, task_id).await?;
		for key in keys {
			let delivered = Message::objects()
				.filter(Message::field_workspace_id().eq(task.workspace_id))
				.filter(
					Message::field_idempotency_key().eq(Some(format!("{peer}:{task_id}:{key}"))),
				)
				.exists_with_db(tx)
				.await?;
			if !delivered
				&& let Some(record) = Self::by_key(tx, task_id, key).await?
				&& record.run_id == run
				&& !record.consumed
				&& (record.expires_at.is_some() || record.input_seq.is_none())
			{
				Self::objects()
					.filter(Self::field_task_id().eq(record.task_id))
					.filter(Self::field_idempotency_key().eq(&record.idempotency_key))
					.delete_with_conn(tx)
					.await?;
			}
		}
		Ok(())
	}

	pub(crate) async fn deliver(
		tx: &mut AtomicTransaction,
		node: &str,
		delivery: RunMessageDelivery<'_>,
	) -> Result<MessageContract> {
		let RunMessageDelivery {
			workspace,
			task_id,
			run_id,
			sender,
			content,
			input_key,
			message_key,
		} = delivery;
		let task = locked_task(tx, task_id).await?;
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		let now = database_time(tx).await?;
		let record = Self::by_key(tx, task_id, input_key)
			.await?
			.filter(|record| record.run_id == run_id)
			.ok_or_else(|| Error::Conflict("remote run message was not reserved at home".into()))?;
		if record.content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if !record.consumed && record.expires_at.is_some_and(|deadline| deadline <= now) {
			return Err(Error::Conflict(
				"remote run message reservation expired".into(),
			));
		}
		if !record.consumed && record.expires_at.is_some() && terminal(&task.status) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"set_config".into_iden(),
				vec![
					Expr::value("aidash.run_message_delivery").into(),
					Expr::value("true").into(),
					Expr::value(true).into(),
				],
			))
			.build(PostgresQueryBuilder);
		TransactionExecutor::execute(tx, &sql, convert_values(values)).await?;
		let message = Message::append_in(tx, node, workspace, sender, content, message_key).await?;
		Self::objects()
			.filter(Self::field_task_id().eq(record.task_id))
			.filter(Self::field_idempotency_key().eq(&record.idempotency_key))
			.update_fields_with_conn(tx, [(Self::field_expires_at(), None::<DateTime<Utc>>)])
			.await?;
		Ok(message)
	}
}

//! Atomic input admission, history import, and message binding.
use super::{Run, RunInput};
use crate::apps::execution::serializers::run_inputs::RunInput as InputContract;
use crate::apps::execution::services::input_ledger::{
	closing, context_size, input_size, reference_size, require_admission,
};
use crate::apps::workspaces::models::{Message, Task, states::TaskStatus};
use crate::apps::workspaces::serializers::entities::{
	Message as MessageContract, Run as RunContract,
};
use crate::domain::nonempty;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

impl RunInput {
	pub(crate) async fn for_run(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
	) -> Result<Vec<InputContract>> {
		Ok(Self::objects()
			.filter(Self::field_run_id().eq(run))
			.order_by(&["seq"])
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.into_iter()
			.map(|record| InputContract {
				seq: record.seq,
				sender: record.sender,
				content: record.content,
				idempotency_key: record.idempotency_key,
				message_id: record.message_id,
				reference_only: record.reference_only,
			})
			.collect())
	}

	async fn with_budget(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		limit: usize,
	) -> Result<(Vec<InputContract>, usize)> {
		let mut inputs = Self::for_run(tx, run).await?;
		let mut used = 0_usize;
		for input in &mut inputs {
			if !input.reference_only
				&& used.saturating_add(input_size(&input.sender, &input.content)) > limit
				&& let Some(message) = input.message_id
			{
				let (sql, values) = Query::update()
					.table(Alias::new("run_inputs"))
					.value_expr(Alias::new("reference_only"), Expr::value(true))
					.and_where(Expr::col("run_id").eq(Expr::value(run)))
					.and_where(Expr::col("idempotency_key").eq(Expr::value(&input.idempotency_key)))
					.and_where(Expr::col("message_id").eq(Expr::value(message)))
					.build(PostgresQueryBuilder);
				tx.execute(&sql, convert_values(values)).await?;
				input.reference_only = true;
			}
			if input.reference_only {
				if input
					.message_id
					.map_or(usize::MAX, |id| reference_size(&input.sender, id))
					> limit
				{
					return Err(Error::Invalid(
						"run message reference exceeds the selected model's input limit".into(),
					));
				}
			} else {
				used = used.saturating_add(context_size(input));
			}
		}
		Ok((inputs, used))
	}

	async fn locked_run(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<RunContract> {
		Run::objects()
			.filter(Run::field_id().eq(run))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::NotFound("run".into()))?
			.try_into()
	}

	pub(crate) async fn admit_with_media(
		tx: &mut dyn TransactionExecutor,
		node: &str,
		run_id: Uuid,
		(sender, content, key): (&str, &str, &str),
		limit: usize,
		media: bool,
	) -> Result<bool> {
		if !media {
			nonempty(content, "message")?;
		}
		let run = Self::locked_run(tx, run_id).await?;
		if let Some(previous) = Self::objects()
			.filter(Self::field_run_id().eq(run_id))
			.filter(Self::field_idempotency_key().eq(key))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
		{
			let has_media = match previous.message_id {
				Some(message) => {
					crate::apps::workspaces::models::ChannelAttachment::has_media_in(tx, message)
						.await?
				}
				None => false,
			};
			return if previous.content == content && has_media == media {
				Ok(true)
			} else {
				Err(Error::Conflict("run message idempotency key reused".into()))
			};
		}
		let task_terminal = Task::objects()
			.filter(Task::field_id().eq(run.task_id))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.first()
			.is_some_and(|task| {
				matches!(
					task.status,
					TaskStatus::Completed
						| TaskStatus::Failed
						| TaskStatus::Cancelled
						| TaskStatus::Abandoned
				)
			});
		require_admission(
			&run,
			task_terminal,
			run.ledger_worker_ready,
			run.lease_owner.is_some(),
		)?;
		if !run.ledger_worker_ready {
			let (sql, values) = Query::update()
				.table(Alias::new("runs"))
				.value_expr(Alias::new("ledger_worker_ready"), Expr::value(true))
				.and_where(Expr::col("id").eq(Expr::value(run_id)))
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
		}
		let (_, used) = Self::with_budget(tx, run_id, limit).await?;
		if used.saturating_add(input_size(sender, content)) > limit {
			return Err(Error::Invalid(
				"run messages exceed the selected model's input limit".into(),
			));
		}
		// PostgreSQL allocates the input sequence; omit its generated key.
		let (sql, values) = Query::insert()
			.into_table(Alias::new("run_inputs"))
			.columns([
				Alias::new("run_id"),
				Alias::new("sender"),
				Alias::new("content"),
				Alias::new("idempotency_key"),
			])
			.values_panic([
				IntoValue::into_value(run_id),
				IntoValue::into_value(sender),
				IntoValue::into_value(content),
				IntoValue::into_value(key),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		if run.home_node == node {
			let message =
				Message::append_in(tx, node, run.workspace_id, sender, content, key).await?;
			Self::bind_message(tx, run_id, key, message.id).await?;
		}
		Ok(false)
	}
	pub(crate) async fn admit(
		tx: &mut dyn TransactionExecutor,
		node: &str,
		run: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<()> {
		Self::admit_with_media(tx, node, run, (sender, content, key), limit, false)
			.await
			.map(|_| ())
	}
	pub(crate) async fn admit_media(
		tx: &mut dyn TransactionExecutor,
		node: &str,
		run: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		limit: usize,
	) -> Result<(bool, InputContract)> {
		let existing =
			Self::admit_with_media(tx, node, run, (sender, content, key), limit, true).await?;
		let input = Self::for_run(tx, run)
			.await?
			.into_iter()
			.find(|input| input.idempotency_key == key)
			.ok_or_else(|| Error::NotFound("run input".into()))?;
		Ok((existing, input))
	}

	pub(crate) async fn import_history(
		tx: &mut dyn TransactionExecutor,
		run_id: Uuid,
		messages: &[(String, MessageContract)],
		limit: usize,
	) -> Result<()> {
		if messages.is_empty() {
			return Ok(());
		}
		// Hold one run lock across the entire history batch and new admission.
		let run = Self::locked_run(tx, run_id).await?;
		for (key, message) in messages {
			let (inputs, used) = Self::with_budget(tx, run_id, limit).await?;
			let previous = inputs.iter().find(|input| input.idempotency_key == *key);
			if previous.is_none() && closing(&run) {
				return Err(Error::Conflict(
					"historical run message arrived after finalization".into(),
				));
			}
			let reference_only = previous.is_none()
				&& used.saturating_add(input_size(&message.sender, &message.content)) > limit;
			if previous.is_none()
				&& reference_only
				&& reference_size(&message.sender, message.id) > limit
			{
				return Err(Error::Invalid(
					"historical run message reference exceeds the selected model's input limit"
						.into(),
				));
			}
			let (sql, values) = Query::insert()
				.into_table(Alias::new("run_inputs"))
				.columns([
					Alias::new("run_id"),
					Alias::new("sender"),
					Alias::new("content"),
					Alias::new("idempotency_key"),
					Alias::new("message_id"),
					Alias::new("reference_only"),
				])
				.values_panic([
					IntoValue::into_value(run_id),
					IntoValue::into_value(&message.sender),
					IntoValue::into_value(&message.content),
					IntoValue::into_value(key),
					IntoValue::into_value(message.id),
					IntoValue::into_value(reference_only),
				])
				.on_conflict(
					OnConflict::columns([Alias::new("run_id"), Alias::new("idempotency_key")])
						.do_nothing()
						.to_owned(),
				)
				.build(PostgresQueryBuilder);
			tx.execute(&sql, convert_values(values)).await?;
			let existing = Self::objects()
				.filter(Self::field_run_id().eq(run_id))
				.filter(Self::field_idempotency_key().eq(key.as_str()))
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("run input".into()))?;
			if existing.content != message.content {
				return Err(Error::Conflict("historical run message key reused".into()));
			}
			Self::bind_message(tx, run_id, key, message.id).await?;
		}
		Ok(())
	}

	pub(crate) async fn bind_message(
		tx: &mut dyn TransactionExecutor,
		run: Uuid,
		key: &str,
		message: Uuid,
	) -> Result<()> {
		let (sql, values) = Query::update()
			.table(Alias::new("run_inputs"))
			.value_expr(Alias::new("message_id"), Expr::value(message))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("idempotency_key").eq(Expr::value(key)))
			.and_where(
				Condition::any()
					.add(Expr::col("message_id").is_null())
					.add(Expr::col("message_id").eq(Expr::value(message))),
			)
			.build(PostgresQueryBuilder);
		if tx
			.execute(&sql, convert_values(values))
			.await?
			.rows_affected
			!= 1
		{
			return Err(Error::Conflict("run input message binding changed".into()));
		}
		Ok(())
	}
}

use reinhardt::query::IntoValue;

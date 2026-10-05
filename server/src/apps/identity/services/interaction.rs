//! Scoped conversation admission and human interaction at the local home node.
use super::{
	access::{Access, NativeAccess},
	identity::SubjectIdentity,
	policy::Resource,
};
use crate::apps::execution::models::HumanRequest as HumanRequestRecord;
use crate::{
	Error, Result, apps::workspaces::serializers::tasks::ConversationResponse, domain::*,
	federation::Federation, registry::EntityRef,
};
use reinhardt::query::{Alias, Condition, Expr, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::{Value, json};
use uuid::Uuid;

impl Access {
	pub(crate) async fn human_visible(&mut self, request: &HumanRequest) -> Result<bool> {
		aidash_application::authorization::visibility::resources::human_visible(
			&mut crate::bootstrap::run_visibility_scope(self),
			request,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool> {
		aidash_application::authorization::visibility::resources::human_reads(
			&mut crate::bootstrap::run_visibility_scope(self),
			workspace,
			run,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn run_for_interaction(&mut self, id: Uuid) -> Result<Run> {
		aidash_application::authorization::workspaces::run_for_interaction(
			&mut crate::bootstrap::run_visibility_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn conversation_resource(
		&mut self,
		conversation: &Conversation,
	) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::conversation_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			conversation,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
		aidash_application::authorization::visibility::resources::human_resource(
			&mut crate::bootstrap::run_visibility_scope(self),
			request,
		)
		.await
		.map_err(Into::into)
	}
}

pub async fn conversation(
	f: &Federation,
	identity: &SubjectIdentity,
	title: &str,
	goal: &str,
	target: &EntityRef,
	target_kind: &str,
) -> Result<ConversationResponse> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = aidash_application::workspaces::conversation(
		&mut crate::bootstrap::conversation_scope(f, &mut access),
		aidash_application::workspaces::ConversationRequest {
			title,
			goal,
			target,
			target_kind,
		},
	)
	.await
	.map_err(Into::into);
	let response = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(response)
}

pub async fn message(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	content: &str,
) -> Result<()> {
	message_keyed(f, identity, id, content, None).await
}
pub async fn message_keyed(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	content: &str,
	key: Option<Uuid>,
) -> Result<()> {
	let key = run_message_key(identity, id, key.unwrap_or_else(Uuid::new_v4));
	// Authorize and inspect the idempotency key before peer calls or registry
	// reads. Both operations use the same access transaction and hide foreign IDs.
	let preflight = {
		let mut access = Access::begin(&f.store, identity).await?;
		let result = async {
			let run = access.run_for_interaction(id).await?;
			access
				.require(&access.resource("run", id, json!({})), "run.message")
				.await?;
			access
				.require(
					&access.resource("workspace", run.workspace_id, json!({})),
					"message.create",
				)
				.await?;
			let previous: Option<(String, Option<Uuid>)> = {
				let query_bind_1 = id;
				let query_bind_2 = &key;
				crate::database::native::query_as(
					&Query::select()
						.columns([Alias::new("content"), Alias::new("message_id")])
						.from(Alias::new("run_inputs"))
						.and_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"run_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"idempotency_key",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
								),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["content", "message_id"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			if previous
				.as_ref()
				.is_some_and(|(previous_content, _)| previous_content != content)
			{
				return Err(Error::Conflict("run message idempotency key reused".into()));
			}
			if let Some(Some(message_id)) = previous.as_ref().map(|(_, id)| id) {
				let attached: Option<Uuid> = {
					let query_bind_1 = message_id;
					crate::database::native::query_scalar(
						&Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("channel_attachments"))
							.and_where(
								Expr::col(Alias::new("message_id"))
									.eq(Expr::value(query_bind_1.to_owned())),
							)
							.limit(1)
							.to_string(PostgresQueryBuilder),
					)
					.scalar_optional(&mut **access.tx)
					.await?
				};
				if attached.is_some() {
					return Err(Error::Conflict(
						"run message idempotency key reused with media".into(),
					));
				}
			}
			Ok((run, previous.map(|(_, message_id)| message_id)))
		}
		.await;
		access.finish(result).await?
	};
	let (run, existing_message) = preflight;
	let home = crate::federation::Home::new(f.clone(), run.clone());
	if existing_message.is_some() {
		if let Err(error) = f.deliver_run_messages(&run).await {
			tracing::warn!(run_id=%id, %error, "accepted scoped run message awaits home delivery");
		}
		f.notify.notify_waiters();
		return Ok(());
	}
	let limit = async {
		f.require_terminal_safe_delivery(&run).await?;
		f.run_message_limit(&run).await
	}
	.await?;
	// Fetch only after the authorized preflight. Import and the new admission
	// then commit under the same run lock as the scoped authorization recheck.
	let history = if home.local() {
		Vec::new()
	} else {
		f.historical_run_message_batch(&run).await?
	};
	if !home.local() && !home.reserve_run_message(&key, content).await? {
		return Err(Error::Conflict(
			"remote home cannot reserve run messages during task termination".into(),
		));
	}
	// Keep the reservation leased while the scoped executor transaction runs.
	// The durable input below is required before promoting the home fence.
	let outcome = async {
		let mut access = Access::begin(&f.store, identity).await?;
		let result = async {
			let current = access.run_for_interaction(id).await?;
			access
				.require(&access.resource("run", id, json!({})), "run.message")
				.await?;
			access
				.require(
					&access.resource("workspace", current.workspace_id, json!({})),
					"message.create",
				)
				.await?;
			Ok(())
		}
		.await;
		if result.is_err() {
			return access.finish(result).await;
		}
		let mut access = access.into_native()?;
		let result = async {
			f.store
				.import_remote_run_messages_in(access.tx.as_mut(), id, &history, limit)
				.await?;
			f.store
				.accept_run_message_in(
					access.tx.as_mut(),
					id,
					&identity.subject,
					content,
					&key,
					limit,
				)
				.await
		}
		.await;
		access.finish(result).await
	}
	.await;
	if let Err(error) = outcome {
		if matches!(error, Error::Conflict(_))
			&& f.recover_historical_run_message(&run, &key, content)
				.await?
		{
			// Historical recovery is an accepted input, so keep its home fence.
		} else {
			if !home.local() {
				match f.store.run_input_sequence(id, &key, content).await {
					Ok(_) => {
						// The admission may have committed even if its client observed
						// a late database error. Keep the fence and recover its sequence.
						home.commit_run_message(&key, content).await?;
						return Ok(());
					}
					Err(Error::Conflict(_)) => {
						home.release_run_messages(std::slice::from_ref(&key))
							.await?;
					}
					Err(check_error) => return Err(check_error),
				}
			}
			return Err(error);
		}
	}
	// The scoped path commits the executor ledger directly instead of going
	// through Federation::admit_run_message, so promote the home fence before
	// treating later delivery as best effort.
	if !home.local() {
		home.commit_run_message(&key, content).await?;
	}
	if let Err(error) = f.deliver_run_messages(&run).await {
		tracing::warn!(run_id=%id, %error, "accepted scoped run message awaits home delivery");
	}
	f.notify.notify_waiters();
	Ok(())
}

pub(crate) fn run_message_key(identity: &SubjectIdentity, id: Uuid, key: Uuid) -> String {
	format!(
		"subject-human:{}:{}:{id}:{key}",
		identity.tenant, identity.subject
	)
}

pub async fn answer(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	response: Value,
) -> Result<HumanRequest> {
	let mut access = NativeAccess::begin(&f.store, identity).await?;
	let result = async {
		let request = HumanRequestRecord::read_in(access.tx.as_mut(), id, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let run = access.run_for_interaction(request.run_id).await?;
		if request.workspace_id != run.workspace_id {
			return Err(Error::Forbidden);
		}
		let resource = access.human_resource(&request).await?;
		access.require(&resource, "human.read").await?;
		access.require(&resource, "human.answer").await?;
		f.store
			.answer_in(access.tx.as_mut(), id, response, &identity.subject)
			.await
	}
	.await;
	let response = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(response)
}

pub async fn abandon(
	f: &Federation,
	identity: &SubjectIdentity,
	id: Uuid,
	revision: i64,
	reason: &str,
) -> Result<Task> {
	let mut access = Access::begin(&f.store, identity).await?;
	let result = async {
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("tasks"))
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
			.fetch_optional(&mut **access.tx)
			.await?
		}
		.ok_or(Error::Forbidden)?;
		let workspace = access.workspace(task.workspace_id).await?;
		access.context = workspace.attributes.clone();
		access.require(&workspace, "workspace.read").await?;
		let resource = access.task_resource(&task).await?;
		access.require(&resource, "task.read").await?;
		access.require(&resource, "task.abandon").await?;
		f.store
			.abandon_task_in(&mut access.tx, id, revision, reason, &identity.subject)
			.await
	}
	.await;
	let task = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(task)
}

use reinhardt::query::ColumnRef;

use reinhardt::query::SimpleExpr;

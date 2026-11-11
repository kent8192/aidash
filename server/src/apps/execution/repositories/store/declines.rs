//! A person answers the tool call an Authority Pause stopped, so the Run goes on without it
//! (#763). Both the local route and a receiver's peer route end here.
use super::Store;
use crate::apps::execution::models::event_records;
use crate::{Error, Result};
use aidash_domain::{
	RawRun, RunControl, RunControlAction, RunInspection,
	run_state::{DeclinedCall, PausedCall, RunState, decode},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

/// What a decline answered, so the caller can tell its own audience.
pub(crate) struct Declined {
	pub(crate) run: RunInspection,
	pub(crate) call: PausedCall,
}

fn not_paused_on_call() -> Error {
	Error::Conflict("the run is not paused on that call".into())
}

impl Store {
	/// Decline the call `key` the Run is paused on. `by` names who, for the Run's own event: the
	/// subject on this Node, or `home` when the Run's Home declined it.
	pub(crate) async fn decline_call_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		key: &str,
		by: &str,
	) -> Result<Declined> {
		let raw: RawRun = aidash_server::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::NotFound("run not found".into()))?;
		if raw.metadata.control != RunControl::Paused || raw.metadata.phase.is_terminal() {
			return Err(not_paused_on_call());
		}
		let (state, recovery) = decode(raw.metadata.phase, raw.pending.clone())?;
		let paused = recovery
			.paused_call
			.clone()
			.filter(|paused| paused.key == key)
			.ok_or_else(not_paused_on_call)?;
		// Only a call that certainly did not run: no row, or a row a refusal left `REFUSED`.
		let status: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("status"))
				.from(Alias::new("invocations"))
				.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		let result = json!({"error": paused.kind.declined_sentence()});
		let workspace = raw
			.metadata
			.is_local(self.deployment.as_str())
			.then_some(raw.metadata.workspace_id);
		match status.as_deref() {
			Some("REFUSED") => {
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("invocations"))
						.value(Alias::new("status"), "COMPLETED")
						.value_expr(Alias::new("result"), Expr::value(result.clone()))
						.value(Alias::new("delivery_pending"), false)
						.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(id)))
						.and_where(Expr::col(Alias::new("idempotency_key")).eq(Expr::value(key)))
						.and_where(Expr::col(Alias::new("status")).eq(Expr::value("REFUSED")))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?;
			}
			None => {
				let RunState::ToolCall(progress) = &state else {
					return Err(not_paused_on_call());
				};
				let (tool, input) = match &paused.tool_call {
					Some(call) if call.name == paused.tool => (&call.name, &call.arguments),
					Some(_) => return Err(not_paused_on_call()),
					None => {
						// Older pauses did not retain the input. Never journal a different tool.
						let call = progress
							.response
							.tool_calls
							.get(paused.index)
							.filter(|call| call.name == paused.tool)
							.ok_or_else(not_paused_on_call)?;
						(&call.name, &call.arguments)
					}
				};
				// Written straight to `COMPLETED`: nothing started, so no `tool.started`.
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("invocations"))
						.columns([
							Alias::new("idempotency_key"),
							Alias::new("run_id"),
							Alias::new("tool"),
							Alias::new("input"),
							Alias::new("status"),
							Alias::new("result"),
							Alias::new("replay_safe"),
							Alias::new("tenant"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(key.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(id).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(tool.clone()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(input.clone()).into()],
								))
								.expr(reinhardt::query::Expr::cust("'COMPLETED'"))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(result.clone()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(paused.replay_safe).into()],
								))
								.expr(crate::database::scope::tenant_of("runs", "id", id))
								.to_owned(),
						)
						.to_owned()
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?;
			}
			// `STARTED`, `UNCERTAIN` and `COMPLETED`: the call may have run.
			Some(_) => return Err(not_paused_on_call()),
		}
		// So the Run's page and stream show the call as answered.
		event_records::append_run(
			&mut **tx,
			id,
			workspace,
			"tool.completed",
			json!({"run_id":id,"idempotency_key":key,"result":result}),
		)
		.await?;
		// Resume through the ordinary control path, which forgets `paused_call`; then keep the
		// decision until the worker has answered the call.
		let inspection = self.control_in(tx, id, RunControlAction::Resume).await?;
		// Only the decisions are written: whatever resuming changed in `pending` stays.
		let mut declined_calls = recovery.declined_calls;
		declined_calls.retain(|declined| declined.key != key);
		declined_calls.push(DeclinedCall {
			key: key.to_owned(),
			kind: paused.kind,
		});
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(
					Alias::new("pending"),
					SimpleExpr::CustomWithExpr(
						"jsonb_set(pending, '{recovery,declined_calls}', ?::jsonb, true)"
							.to_owned(),
						vec![Expr::value(serde_json::to_value(&declined_calls)?).into()],
					),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		// Never the action, never the Home's person.
		event_records::append_run(
			&mut **tx,
			id,
			workspace,
			"run.call_declined",
			json!({
				"run_id": id,
				"task_id": raw.metadata.task_id,
				"tool": paused.tool,
				"kind": paused.kind,
				"call": key,
				"by": by,
			}),
		)
		.await?;
		// #763 item 18: one log line, as for a refusal (#754 item 11).
		tracing::info!(run_id = %id, tool = %paused.tool, kind = ?paused.kind, call = key, "paused call declined");
		Ok(Declined {
			run: inspection,
			call: paused,
		})
	}
}

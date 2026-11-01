//! Authentication refusals checkpoint the unexecuted call, never an uncertain effect.
use super::*;
use aidash_application::connection_wait::{Need, Policy};
use aidash_domain::run_state::{ConnectionCallState, ConnectionWait};

pub(super) struct RefusedCall<'a> {
	pub call: &'a aidash_domain::provider::ToolCall,
	pub tool: &'a dyn ExecutionTool,
	pub guard: Option<&'a dyn ExecutionAuthority>,
	pub key: &'a str,
}

impl Executor<'_> {
	/// Only a fresh typed transport success can retire an outstanding refusal.
	/// Save the fact before publishing the result so a replay never has to infer
	/// provenance from immutable tool JSON (or erase a newer refusal).
	pub(super) async fn checkpoint_connection_success(
		&self,
		run: &mut Run,
		token: Uuid,
		tool: &str,
	) -> Result<()> {
		let before = run.recovery.connection_refusals.len();
		aidash_application::connection_wait::record_success(run, tool);
		if run.recovery.connection_refusals.len() != before {
			// This existing checkpoint adopts the lease-fenced saved projection.
			self.environment.store().checkpoint_mcp(run, token).await?;
		}
		Ok(())
	}
	pub(super) async fn checkpoint_connection_refusal(
		&self,
		run: &mut Run,
		token: Uuid,
		refused: RefusedCall<'_>,
		cause: aidash_domain::agent_plugin::ConnectionCause,
	) -> Result<Value> {
		if !refused.tool.contract().behavior.tenant_tool_call() {
			return Err(Error::Connection(cause));
		}
		let output = serde_json::to_value(&cause)?;
		let state = run
			.state
			.tool_mut()?
			.connection
			.get_or_insert_with(ConnectionCallState::default);
		state.cause = Some(cause.clone());
		state.refused = Some(output.clone());
		state.resolved = false;
		aidash_application::connection_wait::record_refusal(
			run,
			&refused.call.name,
			cause,
			refused.tool.connection_scopes(),
		);
		self.environment
			.store()
			.connection_refused(run, token, refused.key)
			.await?;
		Ok(output)
	}

	pub(super) async fn connection_result(
		&self,
		run: &mut Run,
		token: Uuid,
		refused: RefusedCall<'_>,
		mut output: Value,
	) -> Result<Option<Value>> {
		let RefusedCall {
			call,
			tool,
			guard,
			key,
		} = refused;
		// Only the trusted external-tool contract can turn these codes into a replay.
		if !tool.contract().behavior.tenant_tool_call() {
			return Ok(Some(output));
		}
		let Some(cause) = run
			.state
			.tool()?
			.connection
			.as_ref()
			.and_then(|state| state.cause.as_ref())
		else {
			return Ok(Some(output));
		};
		let Some(mut need) = Need::from_cause(cause, &call.name) else {
			return Ok(Some(output));
		};
		need.scopes.extend(tool.connection_scopes());
		need.scopes.sort();
		need.scopes.dedup();
		let Some(guard) = guard else {
			return Ok(Some(output));
		};
		let Some(mode) = guard.connection_mode(run).await? else {
			return Ok(Some(output));
		};
		if matches!(mode, ApprovalMode::DontAsk | ApprovalMode::Bypass) {
			output["hint"] =
				json!("call connection_request if the task cannot continue without it");
			return Ok(Some(output));
		}
		if run
			.state
			.tool()?
			.connection
			.as_ref()
			.is_some_and(|state| state.resolved)
		{
			return Ok(Some(output));
		}
		guard.action("human.request", "run", run.id).await?;
		let home = self.environment.home(run);
		let request = home
			.human_request(
				run,
				aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND,
				&need.prompt(),
				&format!("{key}:connection"),
			)
			.await?;
		let state = run
			.state
			.tool_mut()?
			.connection
			.get_or_insert_with(ConnectionCallState::default);
		state.refused = Some(output.clone());
		let waited = state.waited_millis.max(
			run.state
				.tool()?
				.mcp
				.as_ref()
				.map_or(0, |checkpoint| checkpoint.waited_millis),
		);
		run.state
			.tool_mut()?
			.connection
			.as_mut()
			.expect("connection state")
			.waited_millis = waited;
		let deadline = Policy {
			answer_deadline_seconds: guard.answer_deadline(run).await?,
			human_wait_seconds: self.environment.mcp_limits().human_wait_seconds,
		}
		.deadline(request.created_at, waited);
		run.recovery.connection_waited.insert(need.resource.clone());
		run.recovery.connection_wait = Some(ConnectionWait {
			request_id: request.id,
			resource: need.resource,
			started_at: request.created_at,
			answer_by: deadline,
			call: None,
			key: key.into(),
		});
		run.state = RunState::Waiting(Box::new(WaitingState::Human {
			request_id: request.id,
			answer_by: Some(deadline),
			resume: ResumeState::ToolCall(Box::new(run.state.tool()?.clone())),
		}));
		self.environment
			.store()
			.save_run(run, token, "run.connection_waiting")
			.await?;
		Ok(None)
	}

	pub(super) async fn builtin_connection_wait(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &aidash_domain::provider::ToolCall,
		request_id: Uuid,
		key: &str,
		guard: Option<&dyn ExecutionAuthority>,
	) -> Result<()> {
		let home = self.environment.home(run);
		let request = home.human_request_by_id(request_id).await?;
		if request.kind != aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND {
			return Err(Error::Invalid("connection request has another kind".into()));
		}
		let payload: Value = serde_json::from_str(&request.prompt)?;
		let resource = aidash_domain::oauth_connections::canonical_resource(
			aidash_application::tools::required(&payload, "resource")?,
		)?;
		let seconds = match guard {
			Some(guard) => guard.answer_deadline(run).await?,
			None => None,
		};
		let deadline = Policy {
			answer_deadline_seconds: seconds,
			human_wait_seconds: self.environment.mcp_limits().human_wait_seconds,
		}
		.deadline(request.created_at, 0);
		run.recovery.connection_waited.insert(resource.clone());
		run.recovery.connection_wait = Some(ConnectionWait {
			request_id,
			resource,
			started_at: request.created_at,
			answer_by: deadline,
			call: Some(call.clone()),
			key: key.into(),
		});
		run.step += 1;
		run.state = RunState::Waiting(Box::new(WaitingState::Human {
			request_id,
			answer_by: Some(deadline),
			resume: ResumeState::Thinking(ThinkingState::default()),
		}));
		self.environment
			.store()
			.save_run(run, token, "run.connection_waiting")
			.await
	}

	pub(super) async fn connection_answer(
		&self,
		run: &mut Run,
		waiting: &mut WaitingState,
		request: Uuid,
		response: &Value,
		guard: Option<&dyn ExecutionAuthority>,
		visibility: &mut dyn ExecutionVisibility,
	) -> Result<bool> {
		let Some(pending) = run
			.recovery
			.connection_wait
			.as_ref()
			.filter(|pending| pending.request_id == request)
			.cloned()
		else {
			return Ok(false);
		};
		let connected = response.get("connected").and_then(Value::as_bool) == Some(true);
		if connected
			&& run.context.plugin_tools.as_ref().is_some_and(|pinned| {
				pinned.findings.iter().any(|finding| {
					finding
						.connection
						.as_ref()
						.is_some_and(|cause| cause.resource() == pending.resource)
				})
			}) {
			if let Some(guard) = guard {
				guard.suspend().await?;
			}
			visibility.suspend().await?;
			let pinned = self
				.environment
				.reconnect_plugin_tools(run, &pending.resource)
				.await;
			let resumed = visibility.resume().await;
			if let Some(guard) = guard {
				guard.resume().await?;
			}
			resumed?;
			if let Some(guard) = guard {
				guard.inference().await?;
			}
			if let Some(pinned) = pinned? {
				run.context.plugin_tools = Some(pinned);
			}
		}
		if let WaitingState::Human {
			resume: ResumeState::ToolCall(resume),
			..
		} = waiting
		{
			let state = resume
				.connection
				.as_mut()
				.ok_or_else(|| Error::Conflict("connection wait lost its logical call".into()))?;
			state.waited_millis = state.waited_millis.saturating_add(
				(chrono::Utc::now().min(pending.answer_by) - pending.started_at)
					.num_milliseconds()
					.max(0) as u64,
			);
			if connected {
				state.attempt = state
					.attempt
					.checked_add(1)
					.ok_or_else(|| Error::Invalid("connection attempts exhausted".into()))?;
				state.refused = None;
				state.cause = None;
				state.resolved = false;
				// The refused admission was removed atomically with its typed fact.
				// The next send gets a fresh attempt under the same logical call.
				resume.mcp = None;
			} else {
				state.resolved = true;
			}
		}
		if let Some(call) = pending.call {
			run.context.push_invocation(
				ContextEvent::tool(call, json!({"connected":connected})),
				&pending.key,
			);
		} else if !matches!(
			waiting,
			WaitingState::Human {
				resume: ResumeState::ToolCall(_),
				..
			}
		) {
			run.context.push(ContextEvent::Human {
				request: format!("Connect to {}", pending.resource),
				request_kind: aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND.into(),
				response: json!({"connected":connected}),
			});
		}
		run.recovery.connection_wait = None;
		run.error = None;
		Ok(true)
	}
}

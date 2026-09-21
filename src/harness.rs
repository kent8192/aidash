use crate::{
	Error, Result,
	authorization::execution::{self, Guard},
	context::{self, Context},
	domain::*,
	federation::{Federation, Home},
	provider::{ModelResponse, provider},
	registry::{AgentConfig, ModelConfig},
	tool::{PluginTool, Tool, ToolConfig, ToolContext, builtins},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Clone)]
pub struct Harness {
	pub federation: Federation,
}
impl Harness {
	pub async fn worker_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let token = Uuid::new_v4();
		let Some(mut run) = store
			.lease_run(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		let result = {
			let work = self.advance(&mut run, token);
			tokio::pin!(work);
			let mut heartbeat = tokio::time::interval(Duration::from_secs(
				(self.federation.config.lease_seconds / 3).max(1) as u64,
			));
			heartbeat.tick().await;
			loop {
				tokio::select! {
					result=&mut work=>break result,
					_=heartbeat.tick()=>{
						if !store.renew_lease(run_id(store,token).await?,token,self.federation.config.lease_seconds).await?{
							return Ok(true);
						}
					}
				}
			}
		};
		if let Err(e) = result {
			let id = match run_id(store, token).await {
				Ok(id) => id,
				Err(_) => return Ok(true),
			};
			let mut current = store.run(id).await?;
			let attempts = current.pending["retry_count"].as_u64().unwrap_or(0) + 1;
			if matches!(e, Error::Forbidden | Error::Unauthorized) {
				store.pause_for_authorization(&current, token).await?;
			} else if matches!(e, Error::TransactionPending) {
				current.pending["retry_at"] =
					json!(chrono::Utc::now() + chrono::Duration::seconds(1));
				store.save_run(&current, token, "run.retrying").await?;
			} else if current.pending.get("terminal_transition").is_some() {
				// Delivery is durable and unbounded; never retry the failed tool
				// just because its home node has not acknowledged terminal state.
				current.pending["last_delivery_error"] = json!(e.to_string());
				current.pending["wake_at"] =
					json!(chrono::Utc::now() + chrono::Duration::seconds(5));
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			} else if matches!(e, Error::External(_))
				&& attempts <= 5
				&& current.control != "CANCELLED"
			{
				current.pending["retry_count"] = json!(attempts);
				current.pending["retry_at"] = json!(
					chrono::Utc::now() + chrono::Duration::seconds(2_i64.pow(attempts as u32))
				);
				current.error = Some(e.to_string());
				store.save_run(&current, token, "run.retrying").await?;
			} else {
				let target = if current.control == "CANCELLED" {
					"CANCELLED"
				} else {
					"FAILED"
				};
				current.phase = "WAITING".into();
				current.pending =
					json!({"terminal_transition":target,"wake_at":chrono::Utc::now()});
				current.error = Some(e.to_string());
				store
					.save_run(&current, token, "run.failure_pending")
					.await?;
			}
		}
		Ok(true)
	}
	pub async fn run_worker(&self) -> Result<()> {
		let (_sender, receiver) = tokio::sync::watch::channel(false);
		self.run_worker_until(receiver).await
	}
	/// Finish the current durable step, then stop claiming work on shutdown.
	pub async fn run_worker_until(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		while !*stopping.borrow() {
			match self.worker_once().await {
				Ok(true) => {}
				Ok(false) => {
					tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=self.federation.notify.notified()=>{}}
				}
				Err(Error::TransactionPending) => {
					tokio::time::sleep(Duration::from_millis(250)).await;
				}
				Err(e) => {
					tracing::error!(error=%e,"worker step failed");
					tokio::time::sleep(Duration::from_secs(1)).await;
				}
			}
		}
		Ok(())
	}
	async fn tool_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &crate::provider::ToolCall,
		cursor: usize,
		message: String,
	) -> Result<()> {
		let mut context: Context = serde_json::from_value(run.context.clone())?;
		context
			.history
			.push(json!({"kind":"tool","call":call,"result":{"error":message}}));
		run.context = json!(context);
		run.pending["cursor"] = json!(cursor + 1);
		self.federation
			.store
			.save_run(run, token, "run.tool_recorded")
			.await?;
		Ok(())
	}

	async fn tools(&self, config: &AgentConfig) -> Result<BTreeMap<String, Arc<dyn Tool>>> {
		let mut tools = builtins();
		for (index, reference) in config.tools.iter().enumerate() {
			let entry = self
				.federation
				.registry
				.get(&reference.id, &reference.version)
				.await?;
			let cfg: ToolConfig = serde_json::from_value(entry.config.clone())?;
			let alias = format!("plugin_{index}");
			tools.insert(
				alias.clone(),
				Arc::new(PluginTool {
					entry,
					alias,
					config: cfg,
					client: self.federation.client.clone(),
				}),
			);
		}
		Ok(tools)
	}
	async fn advance(&self, run: &mut Run, token: Uuid) -> Result<()> {
		if execution::cancel_if_scoped(&self.federation.store, run, token).await? {
			return Ok(());
		}
		let guard = Guard::begin(&self.federation, run).await?;
		let result = self.advance_step(run, token, guard.as_ref()).await;
		if let Some(guard) = guard {
			guard.finish(result).await
		} else {
			result
		}
	}
	async fn advance_step(&self, run: &mut Run, token: Uuid, guard: Option<&Guard>) -> Result<()> {
		let store = &self.federation.store;
		let home = Home::new(self.federation.clone(), run.clone())
			.with_authority(guard.map(Guard::authority));
		let task = home.task().await?;
		if matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		) {
			run.phase = if task.status == "ABANDONED" {
				"CANCELLED".into()
			} else {
				task.status
			};
			run.pending = json!({});
			let kind = if run.phase == "FAILED" {
				"run.failed"
			} else {
				"run.reconciled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let Some(target) = run.pending["terminal_transition"]
			.as_str()
			.map(str::to_owned)
		{
			home.transition(&target).await?;
			run.phase = target;
			run.pending = json!({});
			let kind = if run.phase == "FAILED" {
				"run.failed"
			} else {
				"run.cancelled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let Some(object) = run.pending.as_object_mut() {
			object.remove("retry_at");
			if object.remove("lease_recovered").is_some() {
				let data = json!({"run_id":run.id,"task_id":run.task_id,"phase":run.phase,"cause":"expired worker lease"});
				store
					.emit(
						home.local().then_some(run.workspace_id),
						"run.recovered",
						data.clone(),
					)
					.await?;
				home.report(
					&format!("{}:{}:recovered", run.id, run.revision),
					"remote.run.recovered",
					data,
				)
				.await?;
			}
		}
		if run.control == "CANCELLED" {
			home.transition("CANCELLED").await?;
			run.phase = "CANCELLED".into();
			store.save_run(run, token, "run.cancelled").await?;
			return Ok(());
		}
		let entry = self
			.federation
			.registry
			.get(&run.agent_id, &run.agent_version)
			.await?;
		let agent: AgentConfig = serde_json::from_value(entry.config.clone())?;
		match run.phase.as_str() {
			"READY" => {
				let task = home.task().await?;
				if !task.dependencies.is_empty() {
					let snapshot = home.snapshot().await?;
					if let Some(dependency) = snapshot.tasks.iter().find(|dependency| {
						task.dependencies.contains(&dependency.id)
							&& matches!(
								dependency.status.as_str(),
								"FAILED" | "CANCELLED" | "ABANDONED"
							)
					}) {
						return Err(Error::Invalid(format!(
							"dependency {} is {}",
							dependency.id, dependency.status
						)));
					}
					if task.dependencies.iter().any(|id| {
						!snapshot.tasks.iter().any(|dependency| {
							dependency.id == *id && dependency.status == "COMPLETED"
						})
					}) {
						run.phase = "WAITING".into();
						run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"READY"});
						store.save_run(run, token, "run.waiting").await?;
						return Ok(());
					}
				}
				home.claim(&task, &entry).await?;
				home.transition("RUNNING").await?;
				run.phase = "THINKING".into();
				store.save_run(run, token, "run.started").await?;
			}
			"THINKING" => {
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				if run.step >= agent.max_steps {
					return Err(Error::Invalid("agent max_steps exceeded".into()));
				}
				let model_entry = self
					.federation
					.registry
					.get(&agent.model.id, &agent.model.version)
					.await?;
				let model_cfg: ModelConfig = serde_json::from_value(model_entry.config)?;
				let window = model_cfg.context_window;
				let model = provider(self.federation.client.clone(), model_cfg)?;
				let tools = self.tools(&agent).await?;
				let snapshot = home.snapshot().await?;
				let task = snapshot
					.tasks
					.iter()
					.find(|t| t.id == run.task_id)
					.ok_or_else(|| Error::NotFound("run task".into()))?;
				if task.status == "COMPLETED" {
					run.phase = "COMPLETED".into();
					store.save_run(run, token, "run.recovered").await?;
					return Ok(());
				}
				let mut instructions = crate::context::agent_instructions(&agent.instructions);
				for skill in &agent.skills {
					let skill = self
						.federation
						.registry
						.get(&skill.id, &skill.version)
						.await?;
					if let Some(text) = skill.config["instructions"].as_str() {
						instructions.push('\n');
						instructions.push_str(text);
					}
				}
				let mut context: Context = serde_json::from_value(run.context.clone())?;
				let mut pinned = json!({"identity":{"node_id":self.federation.config.node_id,"agent_id":run.agent_id,"agent_version":run.agent_version},"task":task,"workspace":context::observation::project(&snapshot, 0, context::observation::DEFAULT_LIMIT),"memory":store.memory(run).await?,"agent_state":{"phase":run.phase,"step":run.step}});
				let specifications = tools
					.values()
					.map(|t| t.specification())
					.collect::<Vec<_>>();
				let output = (window / 8).clamp(256, 4096) as u32;
				let budget = context::RequestBudget {
					window,
					instructions: &instructions,
					tools: &specifications,
					max_output_tokens: output,
				};
				// Keep room for history and JSON message escaping. The final fitting
				// decision below measures the complete provider input, not this quota.
				let available = budget.remaining(&Context::default(), &serde_json::Value::Null);
				context::bound_snapshot(&mut pinned, available / 4)?;
				let semantic_budget = budget.remaining(&Context::default(), &pinned) / 2;
				if let Some(guard) = guard {
					if let Some(semantic) = guard
						.semantic_context(
							store,
							&format!("{}\n{}", task.title, task.description),
							semantic_budget,
						)
						.await?
					{
						pinned["semantic_memory"] = json!(semantic);
					}
				} else if home.local() {
					let mut lease = crate::semantic::service::Lease::begin(
						store,
						&crate::authorization::identity::Actor::Operator,
					)
					.await?;
					let result = crate::semantic::service::context_in(
						store,
						&mut lease,
						run,
						&format!("{}\n{}", task.title, task.description),
						semantic_budget,
					)
					.await;
					if let Some(semantic) = lease.finish(result).await? {
						pinned["semantic_memory"] = json!(semantic);
					}
				}

				let compactor: Box<dyn context::jev::JevAsker> = if let Some(guard) = guard {
					Box::new(guard.compactor(&self.federation))
				} else {
					Box::new(context::jev::JevClient::from_env(
						self.federation.client.clone(),
					)?)
				};
				context::compact(&mut context, compactor.as_ref(), &budget, &pinned).await?;
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				let request = budget.request(&context, &pinned);
				crate::generation::budget::Reservation::check_request(window, &request)?;
				let reservation = if let Some(guard) = guard {
					guard
						.reserve_inference(store, token, window, output)
						.await?
				} else {
					None
				};
				let result = model.infer(request).await?;
				if let Some(reservation) = reservation {
					reservation.settle(&result).await?;
				}
				context.usage = json!({"input_tokens":result.input_tokens,"output_tokens":result.output_tokens,"context_window":window,"compactions":context.compactions});
				run.context = json!(context);
				run.pending = json!({"response":result,"cursor":0});
				run.phase = "TOOL_CALL".into();
				run.error = None;
				store.save_run(run, token, "model.completed").await?;
			}
			"TOOL_CALL" => {
				let result: ModelResponse =
					serde_json::from_value(run.pending["response"].clone())?;
				if !result.text.is_empty() {
					if let Some(guard) = guard {
						guard
							.action("message.create", "workspace", run.workspace_id)
							.await?;
					}
					home.message(&format!("{}:{}:output", run.id, run.step), &result.text)
						.await?;
				}
				let cursor = run.pending["cursor"].as_u64().unwrap_or(0) as usize;
				if cursor >= result.tool_calls.len() {
					if result.tool_calls.is_empty() {
						let snapshot = home.snapshot().await?;
						if snapshot.tasks.iter().any(|t| {
							t.parent_id == Some(run.task_id)
								&& !matches!(t.status.as_str(), "COMPLETED" | "ABANDONED")
						}) {
							let failed = snapshot.tasks.iter().any(|t| {
								t.parent_id == Some(run.task_id)
									&& matches!(
										t.status.as_str(),
										"FAILED" | "BLOCKED" | "CANCELLED"
									)
							});
							if failed {
								if let Some(guard) = guard {
									guard.action("human.request", "run", run.id).await?;
								}
								let h=store.human_request(run,"INFORMATION_REQUEST","A subtask needs intervention. You can explicitly abandon failed, blocked or cancelled subtasks in their task details, providing a reason. Then answer this request to continue with the remaining results, or cancel this parent.",&format!("{}:{}:subtasks",run.id,run.step)).await?;
								run.pending =
									json!({"human_request_id":h.id,"resume_phase":"THINKING"});
							} else {
								run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"THINKING"});
							}
							run.step += 1;
							run.phase = "WAITING".into();
							store.save_run(run, token, "run.waiting").await?;
							return Ok(());
						}
						let artifact = ArtifactInput {
							kind: "text".into(),
							name: result_artifact_name(
								snapshot
									.tasks
									.iter()
									.find(|t| t.id == run.task_id)
									.map(|t| t.title.as_str())
									.unwrap_or("Task"),
							),
							content: json!(result.text),
						};
						if let Some(guard) = guard {
							guard
								.action("artifact.create", "artifact", run.task_id)
								.await?;
							guard.action("task.complete", "task", run.task_id).await?;
						}
						if let Err(error) = home
							.complete(&format!("{}:complete", run.id), &artifact)
							.await
						{
							if matches!(error, Error::Conflict(_))
								&& home.snapshot().await?.tasks.iter().any(|task| {
									task.parent_id == Some(run.task_id)
										&& !matches!(
											task.status.as_str(),
											"COMPLETED" | "ABANDONED"
										)
								}) {
								run.step += 1;
								run.phase = "WAITING".into();
								run.pending = json!({"wake_at":chrono::Utc::now()+chrono::Duration::seconds(2),"resume_phase":"THINKING"});
								store.save_run(run, token, "run.waiting").await?;
								return Ok(());
							}
							return Err(error);
						}
						run.phase = "COMPLETED".into();
						store.save_run(run, token, "run.completed").await?;
					} else {
						run.phase = "THINKING".into();
						run.step += 1;
						run.pending = json!({});
						store.save_run(run, token, "run.thinking").await?;
					}
					return Ok(());
				}
				let call = &result.tool_calls[cursor];
				let tools = self.tools(&agent).await?;
				let Some(tool) = tools.get(&call.name) else {
					return self
						.tool_error(
							run,
							token,
							call,
							cursor,
							format!("unavailable tool {}", call.name),
						)
						.await;
				};
				if let Some(guard) = guard {
					match guard.tool(call).await {
						Ok(()) => {}
						Err(Error::Invalid(message)) => {
							return self.tool_error(run, token, call, cursor, message).await;
						}
						Err(error) => return Err(error),
					}
				}
				let key = format!("{}:{}:{}", run.id, run.step, cursor);
				let invocation = store
					.invocation_start(
						run,
						token,
						&key,
						&call.name,
						&call.arguments,
						tool.replay_safe(),
					)
					.await?;
				if invocation.status == "UNCERTAIN" {
					if let Some(guard) = guard {
						guard.action("human.request", "run", run.id).await?;
					}
					store.reconciliation_request(run, token, &key, &format!("Tool {} may have completed before the worker stopped. Reconcile the external effect, then answer with a JSON object containing result. It will not be executed again. Invocation: {key}",call.name)).await?;
					store.save_run(run, token, "run.waiting").await?;
					return Ok(());
				}
				let output = if invocation.status == "COMPLETED" {
					invocation.result.ok_or_else(|| {
						Error::Conflict("completed invocation has no result".into())
					})?
				} else {
					let ctx = ToolContext {
						home: home.clone(),
						store: store.clone(),
						run: run.clone(),
					};
					let output = match tool.invoke(&ctx, call.arguments.clone(), &key).await {
						Ok(output) => output,
						Err(Error::Invalid(message)) => json!({"error":message}),
						Err(e) => return Err(e),
					};
					store.invocation_finish(run, token, &key, &output).await?;
					output
				};
				let mut context: Context = serde_json::from_value(run.context.clone())?;
				context
					.history
					.push(json!({"kind":"tool","call":call,"result":output}));
				run.context = json!(context);
				run.pending["cursor"] = json!(cursor + 1);
				if call.name == "human_request"
					&& let Some(id) = output.get("human_request_id")
				{
					run.pending["human_request_id"] = id.clone();
					run.pending["resume_phase"] = json!("THINKING");
					run.step += 1;
					run.phase = "WAITING".into();
				} else if call.name == "workspace_wait"
					&& let Some(seconds) = output["wait_seconds"].as_i64()
				{
					run.pending["wake_at"] =
						json!(chrono::Utc::now() + chrono::Duration::seconds(seconds));
					run.pending["resume_phase"] = json!("TOOL_CALL");
					run.phase = "WAITING".into();
				}
				home.report(
					&format!("{key}:tool"),
					"remote.tool.completed",
					json!({"run_id":run.id,"call":call,"result":output}),
				)
				.await?;
				store.save_run(run, token, "run.tool_recorded").await?;
			}
			"WAITING" => {
				if let Some(id) = run.pending["human_request_id"].as_str() {
					let id = id
						.parse::<Uuid>()
						.map_err(|_| Error::Invalid("invalid pending human id".into()))?;
					if let Some(guard) = guard {
						guard.human_read(id).await?;
					}
					let h: HumanRequest = sqlx::query_as(
						&sea_orm::sea_query::Query::select()
							.expr(sea_orm::sea_query::SimpleExpr::from(
								sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
							))
							.from(sea_orm::sea_query::Alias::new("human_requests"))
							.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.fetch_one(&store.pool)
					.await?;
					let response = h.response.ok_or_else(|| {
						Error::Conflict("human request has not been answered".into())
					})?;
					if let Some(key) = run.pending["uncertain_key"].as_str() {
						let result = response.get("result").ok_or_else(|| {
							Error::Invalid("reconciliation response must contain result".into())
						})?;
						store.invocation_finish(run, token, key, result).await?;
					} else {
						let mut context: Context = serde_json::from_value(run.context.clone())?;
						context.history.push(
							json!({"kind":"human","request":h.prompt,"request_kind":h.kind,"response":response}),
						);
						run.context = json!(context);
					}
				}
				run.phase = run.pending["resume_phase"]
					.as_str()
					.unwrap_or("THINKING")
					.into();
				for key in [
					"human_request_id",
					"uncertain_key",
					"wake_at",
					"resume_phase",
				] {
					run.pending.as_object_mut().unwrap().remove(key);
				}
				store.save_run(run, token, "run.resumed").await?;
			}
			_ => return Err(Error::Conflict("run is not executable".into())),
		}
		Ok(())
	}
}

async fn run_id(store: &crate::store::Store, token: Uuid) -> Result<Uuid> {
	sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
			))
			.from(sea_orm::sea_query::Alias::new("runs"))
			.and_where(sea_orm::sea_query::Expr::cust("lease_owner = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(token)
	.fetch_optional(&store.pool)
	.await?
	.ok_or_else(|| Error::Conflict("worker lease lost".into()))
}

// Reserve the suffix before truncating, including at a UTF-8 boundary.
fn result_artifact_name(title: &str) -> String {
	let limit = 64_000 - " result".len();
	let mut end = title.len().min(limit);
	while !title.is_char_boundary(end) {
		end -= 1;
	}
	format!("{} result", &title[..end])
}
#[cfg(test)]
mod review_tests {
	#[test]
	fn result_names_fit_for_ascii_and_multibyte_titles() {
		for title in ["a".repeat(64_000), "界".repeat(21_333)] {
			let name = super::result_artifact_name(&title);
			assert!(name.len() <= 64_000);
			assert!(name.ends_with(" result"));
		}
	}
}

//! Scoped home effects share the source authority lease and retry journal.
use super::*;
use crate::domain::{ArtifactInput, NewTask};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	grant_id: Uuid,
	admission_id: Uuid,
	operation: String,
	data: Value,
}

fn field<'a>(data: &'a Value, name: &str) -> Result<&'a str> {
	data[name]
		.as_str()
		.ok_or_else(|| Error::Invalid(format!("missing {name}")))
}

pub(crate) async fn handle(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<Input>,
) -> Result<Json<Value>> {
	let node = crate::api::peer_node(&headers)?;
	let (mut access, d) =
		super::super::description_lease_mode(&f, node, input.grant_id, true).await?;
	access.worker();
	access.read_grant = Some(input.grant_id);
	let result = async {
		let bound = binding(&mut access, input.grant_id)
			.await?
			.ok_or(Error::Forbidden)?;
		if bound.admission_id != input.admission_id {
			return Err(Error::Forbidden);
		}
		if let Some(run) = input.data.get("run_id")
			&& *run != json!(input.admission_id)
		{
			return Err(Error::Forbidden);
		}
		let task = access.task_read(bound.task_id).await?;
		let owner = qualified_agent(node, &d.inspection.agent.id, &d.inspection.agent.version);
		let mutation = matches!(
			input.operation.as_str(),
			"claim"
				| "transition"
				| "run_message_terminal_transition"
				| "run_message_complete"
				| "artifact" | "create_task"
				| "message" | "run_message_output"
				| "event"
		);
		let digest =
			crate::registry::digest(&json!({"operation":input.operation,"data":input.data}));
		let request_key = if let Some(key) = input.data["key"].as_str() {
			if key.is_empty() || key.len() > 512 {
				return Err(Error::Invalid("invalid command key".into()));
			}
			format!("{}:{key}", input.operation)
		} else {
			format!("{}:{}", input.operation, input.data["revision"])
		};
		if mutation {
			let previous: Option<(String, Value)> = sqlx::query_as(
				&Query::select()
					.columns([Alias::new("digest"), Alias::new("result")])
					.from(Alias::new("authorization_remote_commands"))
					.and_where(Expr::cust("grant_id=$1 AND request_key=$2"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(input.grant_id)
			.bind(&request_key)
			.fetch_optional(&mut **access.tx)
			.await?;
			if let Some((old, result)) = previous {
				if old != digest {
					return Err(Error::Conflict(
						"remote command key binds different input".into(),
					));
				}
				return Ok(Json(result));
			}
			if task.owner.as_deref().is_some_and(|v| v != owner) {
				return Err(Error::Forbidden);
			}
			if matches!(
				task.status.as_str(),
				"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
			) {
				return Err(Error::Conflict("remote task is terminal".into()));
			}
		}
		let data = &input.data;
		let key = format!("scoped:{}:{}", input.grant_id, request_key);
		let task_resource = access.task_resource(&task).await?;
		if let Some(tool) = match input.operation.as_str() {
			"artifact" => Some("artifact_publish"),
			"message" => Some("workspace_message"),
			"create_task" => Some("task_create"),
			_ => None,
		} {
			let resource = access.resource("tool", format!("builtin:{tool}"), json!({}));
			access.require(&resource, "tool.invoke").await?;
		}

		let result = match input.operation.as_str() {
			"task" => json!(task),
			"snapshot" => json!(access.workspace_snapshot(task.workspace_id).await?),
			"workspace_record" | "workspace_record_chunk" => {
				let id: Uuid = serde_json::from_value(data["id"].clone())?;
				let kind = field(data, "kind")?;
				let record = access.workspace_record(task.workspace_id, kind, id).await?;
				if input.operation == "workspace_record_chunk" {
					let offset = serde_json::from_value(data["offset"].clone())?;
					let max_chars: usize = serde_json::from_value(data["max_chars"].clone())?;
					if max_chars > 16000 {
						return Err(Error::Invalid(
							"workspace chunk exceeds 16000 characters".into(),
						));
					}
					crate::context::observation::chunk_record(
						record,
						kind,
						&id.to_string(),
						offset,
						max_chars,
					)?
				} else {
					record
				}
			}
			"workspace_children" => {
				let parent: Uuid = serde_json::from_value(data["parent_id"].clone())?;
				if parent != task.id {
					return Err(Error::Forbidden);
				}
				json!(access.workspace_children(task.workspace_id, parent).await?)
			}
			"claim" => {
				access.require(&task_resource, "task.execute").await?;
				if data["entry"] != json!(d.inspection.agent) {
					return Err(Error::Forbidden);
				}
				json!(
					f.store
						.claim_in(
							&mut access.tx,
							&task,
							serde_json::from_value(data["revision"].clone())?,
							&owner,
							&d.inspection.agent
						)
						.await?
				)
			}
			"transition" | "run_message_terminal_transition" => {
				access.require(&task_resource, "task.execute").await?;
				let revision = serde_json::from_value(data["revision"].clone())?;
				let next = field(data, "status")?;
				if input.operation == "run_message_terminal_transition" {
					let through: i64 = serde_json::from_value(data["through_seq"].clone())?;
					if through < 0 {
						return Err(Error::Invalid("invalid terminal sequence".into()));
					}
					json!(
						f.store
							.transition_remote_terminal_in(
								&mut access.tx,
								task.id,
								revision,
								&owner,
								next,
								input.admission_id,
								crate::store::TerminalRunMessageInputs::Through(through)
							)
							.await?
					)
				} else {
					json!(
						f.store
							.transition_in(&mut access.tx, task.id, revision, &owner, next)
							.await?
					)
				}
			}
			"run_message_complete" | "artifact" => {
				let resource = access.artifact_creation_resource(task.id, &owner).await?;
				access.require(&resource, "artifact.create").await?;
				let artifact: ArtifactInput = serde_json::from_value(data["artifact"].clone())?;
				if input.operation == "artifact" {
					let result = f
						.store
						.publish_artifact_in(&mut access.tx, task.id, &owner, &key, &artifact, None)
						.await?;
					record_output(
						&mut access,
						input.grant_id,
						task.workspace_id,
						"artifact",
						result.id,
					)
					.await?;
					json!(result)
				} else {
					access.require(&task_resource, "task.complete").await?;
					let through: i64 = serde_json::from_value(data["through_seq"].clone())?;
					if through < 0 {
						return Err(Error::Invalid("invalid input sequence".into()));
					}
					let result = f
						.store
						.complete_in(
							&mut access.tx,
							task.id,
							&owner,
							&key,
							&artifact,
							None,
							Some((input.admission_id, through)),
						)
						.await?;
					let id: Uuid = sqlx::query_scalar(
						&Query::select()
							.column(Alias::new("id"))
							.from(Alias::new("artifacts"))
							.and_where(Expr::cust("idempotency_key=$1"))
							.to_string(PostgresQueryBuilder),
					)
					.bind(&key)
					.fetch_one(&mut **access.tx)
					.await?;
					record_output(
						&mut access,
						input.grant_id,
						task.workspace_id,
						"artifact",
						id,
					)
					.await?;
					json!(result)
				}
			}
			"create_task" => {
				let resource = access.workspace(task.workspace_id).await?;
				access.require(&resource, "task.create").await?;
				let new: NewTask = serde_json::from_value(data["task"].clone())?;
				if new.parent_id != Some(task.id) {
					return Err(Error::Forbidden);
				}
				for id in &new.dependencies {
					let t = access.task_read(*id).await?;
					if t.workspace_id != task.workspace_id {
						return Err(Error::Forbidden);
					}
				}
				let result = f
					.store
					.create_task_in(&mut access.tx, task.workspace_id, &new, &owner, Some(&key))
					.await?;
				record_output(
					&mut access,
					input.grant_id,
					task.workspace_id,
					"task",
					result.id,
				)
				.await?;
				json!(result)
			}
			"message" | "run_message_output" => {
				let resource = access.workspace(task.workspace_id).await?;
				access.require(&resource, "message.create").await?;
				let content = field(data, "content")?;
				let result = if input.operation == "run_message_output" {
					let through = serde_json::from_value(data["included_input_seq"].clone())?;
					f.store
						.run_message_output_in(
							&mut access.tx,
							crate::store::FencedRunMessageOutput {
								workspace: task.workspace_id,
								task_id: task.id,
								run_id: input.admission_id,
								included_input_seq: through,
								sender: &owner,
								content,
								key: &key,
							},
						)
						.await?
				} else {
					f.store
						.message_in(
							&mut access.tx,
							task.workspace_id,
							&owner,
							content,
							Some(&key),
						)
						.await?
				};
				record_output(
					&mut access,
					input.grant_id,
					task.workspace_id,
					"message",
					result.id,
				)
				.await?;
				json!({"sent":true})
			}
			"run_message_delivery_capability" => json!({"protocol":2}),
			"run_message_reserve" | "run_message_commit" | "run_message_delivery" => {
				access
					.require(
						&access.resource(
							"run",
							input.admission_id,
							task_resource.attributes.clone(),
						),
						"run.message",
					)
					.await?;
				let resource = access.workspace(task.workspace_id).await?;
				access.require(&resource, "message.create").await?;
				let key = field(data, "key")?;
				require_input_key(key, input.admission_id)?;
				let content = field(data, "content")?;
				crate::domain::nonempty(content, "run message")?;
				match input.operation.as_str() {
					"run_message_reserve" => {
						f.store
							.reserve_remote_run_message_in(
								&mut access.tx,
								task.id,
								input.admission_id,
								node,
								key,
								content,
							)
							.await?;
						json!({"reserved":true})
					}
					"run_message_commit" => {
						let seq: i64 = serde_json::from_value(data["input_seq"].clone())?;
						f.store
							.commit_remote_run_message_in(
								&mut access.tx,
								task.id,
								input.admission_id,
								key,
								content,
								Some(seq),
							)
							.await?;
						json!({"committed":true})
					}
					_ => {
						let full_key = format!("{node}:{}:{key}", task.id);
						let message = f
							.store
							.run_message_delivery_in(
								&mut access.tx,
								crate::store::RunMessageDelivery {
									workspace: task.workspace_id,
									task_id: task.id,
									run_id: input.admission_id,
									sender: &format!("human@{node}"),
									content,
									input_key: key,
									message_key: &full_key,
								},
							)
							.await?;
						json!(message)
					}
				}
			}
			"run_message_release" | "run_message_ack" => {
				let keys: Vec<String> = serde_json::from_value(data["keys"].clone())?;
				if keys.len() > 1024 {
					return Err(Error::Invalid("too many input keys".into()));
				}
				for key in &keys {
					require_input_key(key, input.admission_id)?;
				}
				if input.operation == "run_message_release" {
					f.store
						.release_remote_run_message_in(
							&mut access.tx,
							task.id,
							input.admission_id,
							node,
							&keys,
						)
						.await?;
				}
				json!({"acknowledged":true})
			}
			"event" => {
				// Keep audit metadata typed and bound to this admission. Tool
				// arguments/results may include separately protected records,
				// so task events carry identifiers rather than their bytes.
				let payload = &data["data"];
				if payload["run_id"] != json!(input.admission_id) {
					return Err(Error::Forbidden);
				}
				let (kind, detail) = match field(data, "kind")? {
					"remote.run.recovered" => {
						let phase = field(payload, "phase")?;
						let cause = field(payload, "cause")?;
						if phase.len() > 32 || cause != "expired worker lease" {
							return Err(Error::Invalid("invalid recovery event".into()));
						}
						("task.remote_run_recovered", json!({"phase":phase,"cause":cause}))
					}
					"remote.tool.completed" => {
						let call = &payload["call"];
						let name = field(call, "name")?;
						let id = field(call, "id")?;
						if name.len() > 256 || id.len() > 256 {
							return Err(Error::Invalid("invalid tool event".into()));
						}
						("task.remote_tool_completed", json!({"call":{"id":id,"name":name}}))
					}
					_ => return Err(Error::Forbidden),
				};
				f.store.event(
					&mut access.tx,
					Some(task.workspace_id),
					kind,
					json!({"task_id":task.id,"grant_id":input.grant_id,"remote_run_id":input.admission_id,"detail":detail}),
				).await?;
				json!({"recorded":true})
			}
			"run_message_history" => {
				let offset: usize = serde_json::from_value(data["offset"].clone())?;
				let rows: Vec<crate::domain::Message> = sqlx::query_as(
					&Query::select()
						.column(Asterisk)
						.from(Alias::new("messages"))
						.and_where(Expr::cust(
							"workspace_id=$1 AND idempotency_key LIKE $2 AND sender=$3",
						))
						.order_by(Alias::new("created_at"), sea_orm::sea_query::Order::Asc)
						.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
						.limit(4)
						.offset(offset as u64)
						.to_string(PostgresQueryBuilder),
				)
				.bind(task.workspace_id)
				.bind(format!("{node}:{}:%", task.id))
				.bind(format!("human@{node}"))
				.fetch_all(&mut **access.tx)
				.await?;
				let mut visible = Vec::new();
				for message in rows {
					visible.push(
						access
							.workspace_record(task.workspace_id, "message", message.id)
							.await?,
					);
				}
				json!(visible)
			}
			_ => return Err(Error::Invalid("unsupported scoped home operation".into())),
		};
		if mutation {
			let current: i64 = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("revision"))
					.from(Alias::new("tasks"))
					.and_where(Expr::cust("id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(task.id)
			.fetch_one(&mut **access.tx)
			.await?;
			sqlx::query(
				&Query::update()
					.table(Alias::new("authorization_remote_execution"))
					.value(Alias::new("task_revision"), Expr::cust("$2"))
					.and_where(Expr::cust("grant_id=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(input.grant_id)
			.bind(current)
			.execute(&mut **access.tx)
			.await?;
			sqlx::query(
				&Query::insert()
					.into_table(Alias::new("authorization_remote_commands"))
					.columns(["grant_id", "request_key", "digest", "result"].map(Alias::new))
					.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
					.to_string(PostgresQueryBuilder),
			)
			.bind(input.grant_id)
			.bind(request_key)
			.bind(digest)
			.bind(&result)
			.execute(&mut **access.tx)
			.await?;
		}
		if !super::super::live(&mut access, input.grant_id).await? {
			return Err(Error::Forbidden);
		}
		Ok(Json(result))
	}
	.await;
	if let Err(error) = &result {
		tracing::warn!(operation=%input.operation,error=%error,"scoped home command rejected");
	}
	access.finish(result).await
}

async fn record_output(
	access: &mut Access,
	grant: Uuid,
	workspace: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<()> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_remote_outputs"))
			.columns(["grant_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.bind(grant)
	.bind(workspace)
	.bind(kind)
	.bind(id)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}

fn require_input_key(key: &str, run: Uuid) -> Result<()> {
	if key.len() > 512
		|| !(key.starts_with(&format!("human:{run}:"))
			|| (key.starts_with("subject-human:")
				&& key.rsplit(':').nth(1) == Some(run.to_string().as_str())))
	{
		return Err(Error::Invalid("invalid scoped run message key".into()));
	}
	Ok(())
}

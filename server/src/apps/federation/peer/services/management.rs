//! Management use cases.
use crate::{
	Error, Result,
	apps::federation::peer::serializers::mesh::MeshNode,
	apps::federation::peer::serializers::mesh::MeshResponse,
	apps::federation::peer::serializers::mesh::PeerError,
	apps::workspaces::serializers::tasks::PageQuery,
	authorization::{execution, identity::Actor},
	domain::*,
	federation::{Discovery, Federation, Offer, Peer},
	registry::{EntityRef, Entry, Search},
	tool::required,
};
use futures_util::{StreamExt, stream};
use http::HeaderMap;
use reinhardt::injectable;
use reinhardt::query::Alias;
use reinhardt::query::OnConflict;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::{Expr, ExprTrait, LockType};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::apps::federation::peer::serializers::management::RemoteControl;
use crate::apps::federation::peer::serializers::management::WorkspaceCommand;

use crate::apps::identity::services::http_auth::peer_node;
fn run_message_key_matches_run(key: &str, run_id: Uuid) -> bool {
	let run_id_text = run_id.to_string();
	key.starts_with(&format!("human:{run_id}:"))
		|| (key.starts_with("subject-human:")
			&& key.rsplit(':').nth(1) == Some(run_id_text.as_str()))
}
fn is_legacy_run_output_key(key: &str) -> bool {
	let mut parts = key.split(':');
	let (Some(run_id), Some(step), Some("output"), None) =
		(parts.next(), parts.next(), parts.next(), parts.next())
	else {
		return false;
	};
	Uuid::parse_str(run_id).is_ok() && step.parse::<u64>().is_ok()
}
#[derive(Clone)]
pub struct PeerManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerManagement {
	PeerManagement { runtime }
}
impl PeerManagement {
	pub(crate) async fn peer_create(&self, peer: Peer) -> Result<Peer> {
		let f = self.runtime.clone();
		f.register_peer(peer).await
	}
	pub(crate) async fn discover(&self, actor: Actor, query: Search) -> Result<Discovery> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return execution::discover(&f, &identity, &query).await;
		}
		f.discover(&query).await
	}
	pub(crate) async fn mesh(&self) -> Result<MeshResponse> {
		let f = self.runtime.clone();
		let mut nodes = Vec::new();
		let mut errors = Vec::new();
		let peers = f.peers().await?.into_iter().filter(|p| p.enabled);
		let f = &f;
		let mut responses = stream::iter(peers.map(|peer| async move {
			let response = f
				.request::<MeshNode>(&peer.node_id, reqwest::Method::GET, "/observe", None)
				.await;
			(peer, response)
		}))
		.buffer_unordered(8);
		while let Some((peer, response)) = responses.next().await {
			match response {
				Ok(data) if data.node_id == peer.node_id => nodes.push(data),
				Ok(_) => errors.push(PeerError {
					node_id: peer.node_id,
					error: "peer observation node identity mismatch".into(),
				}),
				Err(e) => errors.push(PeerError {
					node_id: peer.node_id,
					error: e.to_string(),
				}),
			}
		}
		Ok(MeshResponse { nodes, errors })
	}
	pub(crate) async fn identity(&self) -> Result<Value> {
		let f = self.runtime.clone();
		Ok(json!(f.config.identity(vec![])))
	}
	pub(crate) async fn peer_discover(
		&self,
		page: PageQuery,
		mut query: Search,
	) -> Result<crate::registry::AgentPage> {
		let f = self.runtime.clone();
		query.kind = Some("agent".into());
		f.registry.legacy_agents(&query, page.offset).await
	}
	pub(crate) async fn peer_agent(&self, (id, version): (String, String)) -> Result<Entry> {
		let f = self.runtime.clone();
		f.store.require_legacy_agent(&id, &version).await?;
		let entry = f.registry.get(&id, &version).await?;
		if entry.kind != "agent" {
			return Err(Error::NotFound("agent".into()));
		}
		Ok(entry)
	}
	pub(crate) async fn peer_offer(&self, headers: HeaderMap, offer: Offer) -> Result<Run> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		let entry = f
			.registry
			.get(&offer.agent.id, &offer.agent.version)
			.await?;
		let query: Search = serde_json::from_value(offer.task.requirements.clone())?;
		if entry.kind != "agent" || !query.matches(&entry) {
			return Err(Error::Invalid(
				"offered task requirements do not match the agent".into(),
			));
		}
		let run = f
			.store
			.accept_run(&offer.task, node, &offer.agent.id, &offer.agent.version)
			.await?;
		f.notify.notify_waiters();
		Ok(run)
	}
	pub(crate) async fn peer_workspace(
		&self,
		headers: HeaderMap,
		command: WorkspaceCommand,
	) -> Result<Value> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		f.authorize_task(node, command.task_id, &command.agent)
			.await?;
		let owner = qualified_agent(node, &command.agent.id, &command.agent.version);
		let task = f.store.task(command.task_id).await?;
		let d = &command.data;
		let key =
			|| -> Result<String> { Ok(format!("{node}:{}:{}", task.id, required(d, "key")?)) };
		if !matches!(
			command.operation.as_str(),
			"snapshot"
				| "snapshot_workspace"
				| "snapshot_page"
				| "workspace_record"
				| "workspace_record_chunk"
				| "workspace_children"
				| "run_message_history"
				| "run_message_delivery_capability"
				| "run_message_reserve"
				| "run_message_commit"
				| "run_message_release"
				| "run_message_ack"
				| "run_message_terminal_transition"
				| "run_message_complete"
				| "task" | "claim"
		) && !(task.owner.is_none()
			&& (matches!(
				command.operation.as_str(),
				"human_message"
					| "run_message_output"
					| "run_message_delivery"
					| "run_message_reserve"
					| "run_message_commit"
					| "run_message_release"
					| "run_message_ack"
					| "run_message_terminal_transition"
					| "run_message_complete"
					| "run_message_history"
					| "run_message_delivery_capability"
			) || (command.operation == "transition"
				&& (d["status"] == "CANCELLED" || d["status"] == "FAILED"))))
			&& task.owner.as_deref() != Some(&owner)
		{
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		) {
			let read = matches!(
				command.operation.as_str(),
				"snapshot"
					| "snapshot_workspace"
					| "snapshot_page"
					| "workspace_record"
					| "workspace_record_chunk"
					| "workspace_children"
					| "run_message_history"
					| "run_message_delivery_capability"
					| "task"
			);
			let replay_completion = matches!(
				command.operation.as_str(),
				"complete" | "run_message_complete"
			) && task.status == crate::domain::TaskStatus::Completed;
			let replay_transition =
				command.operation == "transition" && d["status"] == task.status.as_str();
			let replay_terminal_transition = command.operation == "run_message_terminal_transition"
				&& d["status"] == task.status.as_str();
			if !read
				&& !replay_completion
				&& !replay_transition
				&& !replay_terminal_transition
				&& !matches!(
					command.operation.as_str(),
					"run_message_delivery"
						| "run_message_reserve"
						| "run_message_commit"
						| "run_message_release"
						| "run_message_ack"
				) {
				return Err(Error::Unauthorized);
			}
		}
		let result = match command.operation.as_str() {
			"human_request" | "human_read" | "human_answer" => {
				use crate::apps::identity::repositories::remote_commands::humans::{self, Journal};
				use reinhardt::db::orm::execution::convert_values;
				let run_id: Uuid = serde_json::from_value(d["run_id"].clone())?;
				let mut tx = f.store.database().begin().await?;
				// Fence ownership and liveness with the same Home task lock as transitions.
				let (sql, values) = Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("tasks"))
					.and_where(Expr::col("id").eq(Expr::value(task.id)))
					.and_where(Expr::col("owner").eq(owner.clone()))
					.and_where(Expr::col("status").eq("RUNNING"))
					.lock(LockType::Update)
					.build(PostgresQueryBuilder);
				if tx
					.fetch_optional(&sql, convert_values(values))
					.await?
					.is_none()
				{
					return Err(Error::Forbidden);
				}
				let journal = Journal::Delegation(task.id);
				let request = match command.operation.as_str() {
					"human_request" => {
						humans::create_in(
							tx.as_mut(),
							journal,
							run_id,
							task.workspace_id,
							required(d, "kind")?,
							required(d, "prompt")?,
							&key()?,
						)
						.await?
					}
					"human_answer" => {
						humans::answer_in(
							tx.as_mut(),
							journal,
							run_id,
							serde_json::from_value(d["id"].clone())?,
							d["response"].clone(),
							node,
						)
						.await?
					}
					_ => {
						let id: Uuid = serde_json::from_value(d["id"].clone())?;
						humans::read_in(tx.as_mut(), journal, true)
							.await?
							.into_iter()
							.find(|request| request.id == id && request.run_id == run_id)
							.ok_or(Error::Forbidden)?
					}
				};
				tx.commit().await?;
				json!(request)
			}
			"snapshot" => json!(f.store.snapshot(task.workspace_id).await?),
			"snapshot_workspace" => json!(f.store.workspace(task.workspace_id).await?),
			"snapshot_page" => json!(
				f.store
					.snapshot_page(
						task.workspace_id,
						required(d, "collection")?,
						serde_json::from_value(d["after"].clone())?
					)
					.await?
			),
			"workspace_record" => {
				let id: Uuid = serde_json::from_value(d["id"].clone())?;
				f.store
					.workspace_record(task.workspace_id, required(d, "kind")?, id)
					.await?
			}
			"workspace_record_chunk" => {
				let id: Uuid = serde_json::from_value(d["id"].clone())?;
				let offset: usize = serde_json::from_value(d["offset"].clone())?;
				let max_chars: usize = serde_json::from_value(d["max_chars"].clone())?;
				if max_chars > 16000 {
					return Err(Error::Invalid(
						"workspace record chunk exceeds 16000 characters".into(),
					));
				}
				let kind = required(d, "kind")?;
				let record = f
					.store
					.workspace_record(task.workspace_id, kind, id)
					.await?;
				crate::context::observation::chunk_record(
					record,
					kind,
					&id.to_string(),
					offset,
					max_chars,
				)?
			}
			"workspace_children" => {
				let parent_id: Uuid = serde_json::from_value(d["parent_id"].clone())?;
				if parent_id != task.id {
					return Err(Error::Unauthorized);
				}
				json!(
					f.store
						.child_task_summary(task.workspace_id, parent_id)
						.await?
				)
			}
			"task" => json!(task),
			"claim" => {
				let entry: Entry = serde_json::from_value(d["entry"].clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				if entry.id != command.agent.id
					|| entry.version != command.agent.version
					|| entry.kind != "agent"
				{
					return Err(Error::Unauthorized);
				}
				crate::registry::validate(&entry)?;
				// The peer is authoritative for its advertised agent metadata.
				json!(
					f.store
						.claim(
							task.id,
							d["revision"]
								.as_i64()
								.ok_or_else(|| Error::Invalid("revision required".into()))?,
							&owner,
							&entry
						)
						.await?
				)
			}
			"transition" => json!(
				f.store
					.transition(
						task.id,
						d["revision"]
							.as_i64()
							.ok_or_else(|| Error::Invalid("revision required".into()))?,
						&owner,
						serde_json::from_value(d["status"].clone())
							.map_err(|_| Error::Invalid("invalid task status".into()))?
					)
					.await?
			),
			"run_message_terminal_transition" => {
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let revision = d["revision"]
					.as_i64()
					.ok_or_else(|| Error::Invalid("revision required".into()))?;
				let status = serde_json::from_value(d["status"].clone())
					.map_err(|_| Error::Invalid("invalid task status".into()))?;
				if let Some(value) = d.get("through_seq") {
					let through_seq = value
						.as_i64()
						.ok_or_else(|| Error::Invalid("invalid terminal input sequence".into()))?;
					json!(
						f.store
							.transition_remote_run_message_terminal_through(
								task.id,
								revision,
								&owner,
								status,
								run_id,
								through_seq,
							)
							.await?
					)
				} else {
					let keys: Vec<String> = serde_json::from_value(d["keys"].clone())?;
					if keys.len() > 1024
						|| keys
							.iter()
							.any(|key| !run_message_key_matches_run(key, run_id))
					{
						return Err(Error::Invalid("invalid run message keys".into()));
					}
					json!(
						f.store
							.transition_remote_run_message_terminal(
								task.id, revision, &owner, status, run_id, &keys,
							)
							.await?
					)
				}
			}
			"complete" => json!(
				f.store
					.complete(
						task.id,
						&owner,
						&key()?,
						&serde_json::from_value::<ArtifactInput>(d["artifact"].clone())?
					)
					.await?
			),
			"run_message_complete" => {
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let input_key = required(d, "key")?;
				if !input_key.starts_with(&format!("{run_id}:")) {
					return Err(Error::Invalid("invalid run completion key".into()));
				}
				let through_seq = d["through_seq"]
					.as_i64()
					.ok_or_else(|| Error::Invalid("invalid terminal input sequence".into()))?;
				let artifact = serde_json::from_value::<ArtifactInput>(d["artifact"].clone())?;
				json!(
					f.store
						.complete_remote_run_message(
							task.id,
							&owner,
							&key()?,
							&artifact,
							run_id,
							through_seq,
						)
						.await?
				)
			}
			"artifact" => json!(
				f.store
					.publish_artifact(
						task.id,
						&owner,
						&key()?,
						&serde_json::from_value::<ArtifactInput>(d["artifact"].clone())?
					)
					.await?
			),
			"create_task" => json!(
				f.store
					.create_task(
						task.workspace_id,
						&serde_json::from_value::<NewTask>(d["task"].clone())?,
						&owner,
						Some(&key()?)
					)
					.await?
			),
			"delegate" => {
				let child_id = required(d, "task_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid task ID".into()))?;
				let child = f.store.task(child_id).await?;
				if child.workspace_id != task.workspace_id {
					return Err(Error::Unauthorized);
				}
				json!(
					f.delegate(
						child_id,
						required(d, "node_id")?,
						&serde_json::from_value::<EntityRef>(d["agent"].clone())?
					)
					.await?
				)
			}
			"message" | "human_message" => {
				if command.operation == "message" && is_legacy_run_output_key(required(d, "key")?) {
					return Err(Error::Conflict(
						"run outputs require fenced response publication".into(),
					));
				}
				let sender = if command.operation == "human_message" {
					format!("human@{node}")
				} else {
					owner.clone()
				};
				let message = f
					.store
					.message_record(
						task.workspace_id,
						&sender,
						required(d, "content")?,
						Some(&key()?),
					)
					.await?;
				if command.operation == "human_message" {
					json!(message)
				} else {
					json!({"sent":true})
				}
			}
			"run_message_output" => {
				f.store.require_legacy_execution(task.workspace_id).await?;
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let input_key = required(d, "key")?;
				if !input_key.starts_with(&format!("{run_id}:")) {
					return Err(Error::Invalid("invalid run response key".into()));
				}
				let content = required(d, "content")?;
				let message_key = key()?;
				if let Some(included_input_seq) = d.get("included_input_seq") {
					let included_input_seq = included_input_seq
						.as_i64()
						.ok_or_else(|| Error::Invalid("invalid included input sequence".into()))?;
					f.store
						.run_message_output_record_fenced(crate::store::FencedRunMessageOutput {
							workspace: task.workspace_id,
							task_id: task.id,
							run_id,
							included_input_seq,
							sender: &owner,
							content,
							key: &message_key,
						})
						.await?;
				} else {
					f.store
						.run_message_output_record(
							task.workspace_id,
							task.id,
							run_id,
							&owner,
							content,
							&message_key,
						)
						.await?;
				}
				json!({"sent":true})
			}
			"run_message_delivery" => {
				// authorize_task above validates this peer's delegation; the run ID
				// and idempotency key below fence delivery to the delegated task.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let input_key = required(d, "key")?;
				if !run_message_key_matches_run(input_key, run_id) {
					return Err(Error::Invalid("invalid run message delivery key".into()));
				}
				let content = required(d, "content")?;
				json!(
					f.store
						.run_message_delivery_record(crate::store::RunMessageDelivery {
							workspace: task.workspace_id,
							task_id: task.id,
							run_id,
							sender: &format!("human@{node}"),
							content,
							input_key,
							message_key: &key()?,
						})
						.await?
				)
			}
			"run_message_reserve" => {
				// The peer task grant above supplies scoped execution authority; this
				// run-bound key preserves the durable admission fence.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let input_key = required(d, "key")?;
				if !run_message_key_matches_run(input_key, run_id) {
					return Err(Error::Invalid("invalid run message key".into()));
				}
				f.store
					.reserve_remote_run_message(
						task.id,
						run_id,
						node,
						input_key,
						required(d, "content")?,
					)
					.await?;
				json!({"reserved":true})
			}
			"run_message_commit" => {
				// Commit is limited to a previously delegated task and its run-bound
				// admission key.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let input_key = required(d, "key")?;
				if !run_message_key_matches_run(input_key, run_id) {
					return Err(Error::Invalid("invalid run message key".into()));
				}
				let input_seq = d
					.get("input_seq")
					.map(|value| {
						value
							.as_i64()
							.ok_or_else(|| Error::Invalid("invalid admitted input sequence".into()))
					})
					.transpose()?;
				f.store
					.commit_remote_run_message_with_sequence(
						task.id,
						run_id,
						input_key,
						required(d, "content")?,
						input_seq,
					)
					.await?;
				json!({"committed":true})
			}
			"run_message_release" => {
				// Only this peer's delegated task can release run-bound reservations.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let keys: Vec<String> = serde_json::from_value(d["keys"].clone())?;
				if keys.len() > 1024
					|| keys
						.iter()
						.any(|key| !run_message_key_matches_run(key, run_id))
				{
					return Err(Error::Invalid("invalid run message keys".into()));
				}
				f.store
					.release_remote_run_message(task.id, run_id, node, &keys)
					.await?;
				json!({"released":true})
			}
			"run_message_ack" => {
				// Only this peer's delegated task can acknowledge run-bound inputs.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let keys: Vec<String> = serde_json::from_value(d["keys"].clone())?;
				if keys.len() > 1024
					|| keys
						.iter()
						.any(|key| !run_message_key_matches_run(key, run_id))
				{
					return Err(Error::Invalid("invalid run message keys".into()));
				}
				f.store
					.acknowledge_remote_run_messages(task.id, run_id, &keys)
					.await?;
				json!({"acknowledged":true})
			}
			"run_message_history" => {
				// authorize_task above scopes history reads to the delegated task;
				// query predicates below also bind records to that task and run.
				let run_id: Uuid = required(d, "run_id")?
					.parse()
					.map_err(|_| Error::Invalid("invalid run ID".into()))?;
				let offset = d["offset"]
					.as_u64()
					.ok_or_else(|| Error::Invalid("offset required".into()))?;
				let messages = f
					.store
					.remote_run_message_history(task.workspace_id, node, task.id, run_id, offset)
					.await?;
				json!(messages)
			}
			"run_message_delivery_capability" => {
				// This is a capability read for the already authorized delegated task.
				json!({"protocol":2})
			}
			"event" => {
				let event_id =
					crate::registry::digest(&json!({"node":node,"task":task.id,"key":key()?}));
				let mut tx = crate::database::native::begin(&f.store.pool).await?;
				// A deterministic UUID-sized identifier is enough for the inbox key;
				// the full source namespace is included in the hash.
				use sha2::{Digest, Sha256};
				let hash = Sha256::digest(event_id.as_bytes());
				let mut bytes = [0; 16];
				bytes.copy_from_slice(&hash[..16]);
				let id = Uuid::from_bytes(bytes);
				let inserted = {
					let query_bind_1 = node;
					let query_bind_2 = id;
					crate::database::native::query(
						&Query::insert()
							.into_table(Alias::new("peer_events"))
							.columns([Alias::new("node_id"), Alias::new("event_id")])
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
									.to_owned(),
							)
							.on_conflict(
								OnConflict::columns(["node_id", "event_id"])
									.do_nothing()
									.to_owned(),
							)
							.to_string(PostgresQueryBuilder),
					)
					.execute(&mut *tx)
					.await?
				}
				.rows_affected();
				if inserted > 0 {
					f.store
						.event(
							&mut tx,
							Some(task.workspace_id),
							"federation.event",
							json!({"node_id":node,"task_id":task.id,"kind":required(d,"kind")?,"data":d["data"]}),
						)
						.await?;
				}
				tx.commit().await?;
				json!({"received":true})
			}
			_ => return Err(Error::Invalid("unknown federation operation".into())),
		};
		Ok(result)
	}
	pub(crate) async fn peer_observe(&self, headers: HeaderMap) -> Result<Value> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		Ok(serde_json::to_value(f.store.peer_observation(node).await?)?)
	}

	pub(crate) async fn peer_control(
		&self,
		headers: HeaderMap,
		input: RemoteControl,
	) -> Result<Value> {
		let f = self.runtime.clone();
		let node = peer_node(&headers)?;
		let run = f.store.run(input.run_id).await?;
		if run.home_node != node {
			return Err(Error::Unauthorized);
		}
		match input.action.as_str() {
			"answer" => {
				let id = input
					.request_id
					.ok_or_else(|| Error::Invalid("request_id required".into()))?;
				let valid: bool = {
					let query_bind_1 = id;
					let query_bind_2 = run.id;
					crate::database::native::query_scalar(&Query::select()
						.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM human_requests WHERE id = ? AND run_id = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
						.to_string(PostgresQueryBuilder))
				.scalar_one(&f.store.pool)
				.await?
				};
				if !valid {
					return Err(Error::Unauthorized);
				}
				let response = input
					.response
					.ok_or_else(|| Error::Invalid("response required".into()))?;
				if crate::authorization::peer::admission::run_grant(&f.store, &run.metadata())
					.await?
					.is_none()
				{
					crate::federation::Home::new(f.clone(), run.clone())
						.answer_home_human(id, response.clone())
						.await?;
				}
				Ok(json!(f.store.answer(id, response).await?))
			}
			"message" => {
				let content = input
					.content
					.ok_or_else(|| Error::Invalid("content required".into()))?;
				let key = format!(
					"human:{}:{}",
					run.id,
					input.idempotency_key.unwrap_or_else(Uuid::new_v4)
				);
				let limit = f.run_message_limit(&run).await?;
				f.require_terminal_safe_delivery(&run).await?;
				f.admit_run_message(&run, "human", &content, &key, limit)
					.await?;
				if let Err(error) = f.deliver_run_messages(&run).await {
					tracing::warn!(run_id=%run.id, %error, "accepted remote-control message awaits home delivery");
				}
				f.notify.notify_waiters();
				Ok(json!({"sent":true}))
			}
			action => Ok(json!(
				f.store
					.control(
						run.id,
						serde_json::from_value(json!(action))
							.map_err(|_| Error::Invalid("invalid run control action".into()))?
					)
					.await?
			)),
		}
	}
}

use reinhardt::query::SimpleExpr;

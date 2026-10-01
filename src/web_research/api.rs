use super::{contracts::*, evidence, persistence, service};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	domain::Run,
	federation::Federation,
};
use axum::{
	Extension, Json,
	extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Usage {
	pub run_id: Uuid,
	pub search_attempts: u64,
	pub page_attempts: u64,
	pub estimated_micro_usd: u64,
	pub cache_bytes: u64,
	pub observation_count: u64,
	pub observation_bytes: u64,
	pub classification: String,
	pub context_digest: String,
	pub enabled: bool,
	pub search_available: bool,
	pub page_extractor_configured: bool,
	pub account_month_estimate_micro_usd: Option<i64>,
	pub node_month_estimate_micro_usd: i64,
	pub search_limit: u64,
	pub page_limit: u64,
	pub monthly_limit_micro_usd: u64,
	pub disclosures: Vec<Value>,
	pub uncertain_operations: Vec<Uuid>,
}
pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(usage))
		.routes(routes!(evidence_read))
		.routes(routes!(decision))
		.routes(routes!(classify))
		.routes(routes!(revoke))
		.routes(routes!(start))
		.routes(routes!(thread_runs))
}
#[utoipa::path(post,path="/workspaces/{workspace}/threads/{thread}/web/runs",operation_id="web_thread_start",params(("workspace"=Uuid,Path),("thread"=Uuid,Path)),request_body=Start,responses((status=200,body=Run)),security(("bearer_auth"=[])))]
async fn start(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
	Json(input): Json<Start>,
) -> Result<Json<Run>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		if !f.store.web.profile.admission {
			return Err(Error::Conflict("web_research_disabled".into()));
		}
		if input.description.trim().is_empty() || input.description.len() > 16384 {
			return Err(Error::Invalid("invalid research request".into()));
		}
		let channel: crate::collaboration::ChannelThread = sqlx::query_as(
			&persistence::select("channel_threads")
				.and_where(sea_orm::sea_query::Expr::cust("id=$1 AND workspace_id=$2"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(thread)
		.bind(workspace)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
		crate::capabilities::thread_lifecycle::visible(&mut access.tx, thread).await?;
		access
			.workspace_record(workspace, "message", channel.root_message_id)
			.await?;
		let digest =
			crate::registry::digest(&json!(["web-thread-start/1", workspace, thread, input]));
		if let Some(result) =
			crate::capabilities::sessions::cached(&mut access, input.idempotency_key, &digest)
				.await?
		{
			let id: Uuid = serde_json::from_value(result["run_id"].clone())?;
			return access.run_for_interaction(id).await;
		}
		let entry =
			crate::authorization::catalog::entry(&mut access, &input.agent, "agent.execute")
				.await?;
		let config: crate::registry::AgentConfig = serde_json::from_value(entry.config)?;
		if !(config.core_capabilities.web_search
			|| config.core_capabilities.web_open
			|| config.core_capabilities.web_find)
		{
			return Err(Error::Invalid("agent_web_capabilities_disabled".into()));
		}
		access
			.require(
				&access.resource("workspace", workspace, json!({})),
				"task.create",
			)
			.await?;
		let requester = access.identity.subject.clone();
		let open_url = input
			.open_url
			.as_deref()
			.map(super::network::url)
			.transpose()?;
		let description = if let Some(url) = &open_url {
			format!(
				"{}\n\nOpen this exact public URL: {}",
				input.description, url
			)
		} else {
			input.description.clone()
		};
		let task = f
			.store
			.create_task_in(
				&mut access.tx,
				workspace,
				&crate::domain::NewTask {
					title: "Web research".into(),
					description: description.clone(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				&requester,
				None,
			)
			.await?;
		crate::capabilities::sessions::bind_task(&mut access, task.id, thread).await?;
		crate::authorization::execution::delegate_in(&f, &mut access, task.id, &input.agent)
			.await?;
		let run: Run = sqlx::query_as(
			&persistence::select("runs")
				.and_where(sea_orm::sea_query::Expr::cust("task_id=$1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task.id)
		.fetch_one(&mut **access.tx)
		.await?;
		persistence::message_dependency(&mut access, &run, channel.root_message_id).await?;
		let mut state = persistence::state(&mut access, &run).await?;
		state["thread_id"] = json!(thread);
		persistence::save_state(&mut access, &run, &state).await?;
		if let Some(url) = open_url {
			access
				.require(
					&access.resource("workspace", workspace, json!({})),
					"message.create",
				)
				.await?;
			let key = format!("{}:web-url-request:{}", run.id, input.idempotency_key);
			f.store
				.accept_run_message_in(&mut access.tx, run.id, &requester, &description, &key, 8192)
				.await?;
			let message: crate::domain::Message = sqlx::query_as(
				&persistence::select("messages")
					.and_where(sea_orm::sea_query::Expr::cust(
						"workspace_id=$1 AND idempotency_key=$2",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(workspace)
			.bind(key)
			.fetch_one(&mut **access.tx)
			.await?;
			service::attach_response(&mut access.tx, &run, &message).await?;
			service::url_intent(
				&mut access,
				&run,
				url.as_str(),
				message.id,
				input.idempotency_key,
			)
			.await?;
		}
		crate::capabilities::sessions::cache(
			&mut access,
			input.idempotency_key,
			&digest,
			&json!({"run_id":run.id}),
		)
		.await?;
		Ok(run)
	}
	.await;
	let result = access.finish(result).await?;
	f.notify.notify_waiters();
	Ok(Json(result))
}
#[utoipa::path(get,path="/workspaces/{workspace}/threads/{thread}/web/runs",operation_id="web_thread_runs",params(("workspace"=Uuid,Path),("thread"=Uuid,Path)),responses((status=200,body=Vec<Run>)),security(("bearer_auth"=[])))]
async fn thread_runs(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((workspace, thread)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Run>>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		crate::capabilities::thread_lifecycle::visible(&mut access.tx, thread).await?;
		let root: Uuid = sqlx::query_scalar(
			&persistence::select("channel_threads")
				.clear_selects()
				.column(sea_orm::sea_query::Alias::new("root_message_id"))
				.and_where(sea_orm::sea_query::Expr::cust("id=$1 AND workspace_id=$2"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(thread)
		.bind(workspace)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
		access.workspace_record(workspace, "message", root).await?;
		let workspace_resource = access.workspace(workspace).await?;
		access
			.require(&workspace_resource, "workspace.read")
			.await?;
		let runs: Vec<Run> = sqlx::query_as(
			&persistence::select("runs")
				.and_where(sea_orm::sea_query::Expr::cust(
					"workspace_id=$1 AND id IN (SELECT run_id FROM web_runs WHERE data->>'thread_id'=$2)",
				))
				.order_by(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Order::Desc,
				)
				.limit(100)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(thread.to_string())
		.fetch_all(&mut **access.tx)
		.await?;
		let mut visible = vec![];
		for run in runs {
			if access.run_visible(&run).await? {
				visible.push(run);
			}
		}
		Ok(visible)
	}
	.await;
	Ok(Json(access.finish(result).await?))
}
async fn access(f: &Federation, actor: Actor, id: Uuid) -> Result<(Access, Run)> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let run = access.run_for_interaction(id).await?;
	if run.home_node != f.store.node_id || !access.run_visible(&run).await? {
		return Err(Error::NotFound("run unavailable".into()));
	}
	Ok((access, run))
}
#[utoipa::path(get,path="/runs/{id}/web/usage",operation_id="web_run_usage",params(("id"=Uuid,Path)),responses((status=200,body=Usage)),security(("bearer_auth"=[])))]
async fn usage(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Usage>> {
	let (mut access, run) = access(&f, actor, id).await?;
	let result=async {
		let state=persistence::state(&mut access,&run).await?;
		let context_digest=persistence::fingerprint(&mut access,&run).await?;
		let classification=if state["context_digest"]==context_digest {state["classification"].as_str().unwrap_or("unclassified")} else {"unclassified"}.to_owned();
		let operations:Vec<crate::capabilities::records::Record>=sqlx::query_as(&persistence::select("core_records")
			.and_where(sea_orm::sea_query::Expr::cust("tenant=$1 AND kind='web.operation' AND data->>'run_id'=$2 AND state IN ('pending','dispatched','uncertain')"))
			.order_by(sea_orm::sea_query::Alias::new("id"),sea_orm::sea_query::Order::Asc).limit(201).to_string(sea_orm::sea_query::PostgresQueryBuilder))
			.bind(&access.identity.tenant).bind(run.id.to_string()).fetch_all(&mut **access.tx).await?;
		let uncertain_operations=operations.iter().filter(|record|record.state=="uncertain").map(|record|record.id).collect();
		let disclosures=operations.into_iter().filter(|record|record.state=="pending" && !persistence::expired(record))
			.filter(|record| record.data["approver"]==access.identity.subject || record.owner==access.identity.subject)
			.map(|record| service::approval_view(&record)["data"].clone()).collect();
		let account_month_estimate_micro_usd=if let Some(account)=&f.store.web.account {
			sqlx::query_scalar(&sea_orm::sea_query::Query::select().column(sea_orm::sea_query::Alias::new("used_micro_usd"))
				.from(sea_orm::sea_query::Alias::new("web_months")).and_where(sea_orm::sea_query::Expr::cust("node_id=$1 AND account_id=$2 AND month=$3"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(&f.store.node_id).bind(&account.account_id)
				.bind(chrono::Utc::now().format("%Y-%m").to_string()).fetch_optional(&mut **access.tx).await?
		} else {None};
		let node_month_estimate_micro_usd=sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("COALESCE(SUM(used_micro_usd),0)::bigint"))
			.from(sea_orm::sea_query::Alias::new("web_months")).and_where(sea_orm::sea_query::Expr::cust("node_id=$1 AND month=$2"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(&f.store.node_id)
			.bind(chrono::Utc::now().format("%Y-%m").to_string()).fetch_one(&mut **access.tx).await?;
		Ok(Usage {run_id:id,search_attempts:state["search_attempts"].as_u64().unwrap_or(0),page_attempts:state["page_attempts"].as_u64().unwrap_or(0),
			estimated_micro_usd:state["estimated_micro_usd"].as_u64().unwrap_or(0),cache_bytes:state["cache_bytes"].as_u64().unwrap_or(0),
			observation_count:state["observation_count"].as_u64().unwrap_or(0),observation_bytes:state["observation_bytes"].as_u64().unwrap_or(0),
			classification,context_digest,enabled:f.store.web.profile.admission,search_available:f.store.web.search_available(),
			page_extractor_configured:f.store.capabilities.0.runner.is_some(),account_month_estimate_micro_usd,node_month_estimate_micro_usd,
			search_limit:f.store.web.profile.search_attempt_limit,page_limit:f.store.web.profile.page_attempt_limit,monthly_limit_micro_usd:f.store.web.account.as_ref().map_or(20_000_000,|a|a.monthly_limit_micro_usd.min(20_000_000)),disclosures,uncertain_operations})
	}.await;
	Ok(Json(access.finish(result).await?))
}
#[utoipa::path(get,path="/runs/{id}/web/evidence/{reference}",operation_id="web_evidence_read",params(("id"=Uuid,Path),("reference"=String,Path)),responses((status=200,body=Value)),security(("bearer_auth"=[])))]
async fn evidence_read(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, reference)): Path<(Uuid, String)>,
) -> Result<Json<Value>> {
	let (mut access, run) = access(&f, actor, id).await?;
	let result = evidence::get(&mut access, &run, &reference)
		.await
		.map(|record| record.data);
	Ok(Json(access.finish(result).await?))
}
#[utoipa::path(post,path="/runs/{id}/web/disclosures/{approval}/decision",operation_id="web_disclosure_decide",params(("id"=Uuid,Path),("approval"=Uuid,Path)),request_body=DisclosureDecision,responses((status=200,body=Value)),security(("bearer_auth"=[])))]
async fn decision(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((id, approval)): Path<(Uuid, Uuid)>,
	Json(input): Json<DisclosureDecision>,
) -> Result<Json<Value>> {
	let (mut access, run) = access(&f, actor, id).await?;
	let result = service::decide(&f.store, &mut access, &run, approval, input).await;
	Ok(Json(access.finish(result).await?))
}
#[utoipa::path(post,path="/runs/{id}/web/context",operation_id="web_context_classify",params(("id"=Uuid,Path)),request_body=ContextLabel,responses((status=200,body=Value)),security(("bearer_auth"=[])))]
async fn classify(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<ContextLabel>,
) -> Result<Json<Value>> {
	let (mut access, run) = access(&f, actor, id).await?;
	let result = async {
		if !service::active(&run) {
			return Err(Error::Conflict("run is terminal".into()));
		}
		// Authenticated owner action only: never a tool or a model-controlled label.
		crate::authorization::execution::inherit_run_authority(&mut access, &run).await?;
		if access
			.snapshot
			.bundle
			.subjects
			.get(&access.identity.subject)
			.is_none_or(|s| s.kind != crate::authorization::policy::SubjectKind::User)
		{
			return Err(Error::Forbidden);
		}
		access
			.require(&access.resource("run", id, json!({})), "web.disclose")
			.await?;
		let digest = persistence::fingerprint(&mut access, &run).await?;
		if input.expected_digest != digest {
			return Err(Error::Conflict("context changed".into()));
		}
		let mut state = persistence::state(&mut access, &run).await?;
		state["classification"] = json!(if input.public { "public" } else { "nonpublic" });
		state["context_digest"] = json!(digest);
		state["classified_by"] = json!(access.identity.subject);
		state["classified_at"] = json!(chrono::Utc::now());
		persistence::save_state(&mut access, &run, &state).await?;
		f.store
			.event(
				&mut access.tx,
				Some(run.workspace_id),
				"web.context_classified",
				json!({"run_id":id,"public":input.public,"actor":state["classified_by"],"digest":digest}),
			)
			.await?;
		Ok(json!({"classification":state["classification"],"context_digest":digest}))
	}
	.await;
	Ok(Json(access.finish(result).await?))
}
#[utoipa::path(post,path="/runs/{id}/web/revoke",operation_id="web_evidence_revoke",params(("id"=Uuid,Path)),request_body=Revision,responses((status=200,body=Value)),security(("bearer_auth"=[])))]
async fn revoke(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<Revision>,
) -> Result<Json<Value>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin_exclusive(&f.store, &identity).await?;
	let run = access.run_for_interaction(id).await?;
	if !access.run_visible(&run).await? {
		return Err(Error::NotFound("run unavailable".into()));
	}
	let result = async {
		crate::authorization::execution::inherit_run_authority(&mut access, &run).await?;
		access
			.require(&access.resource("run", id, json!({})), "web.revoke")
			.await?;
		if run.revision != input.expected_revision {
			return Err(Error::Conflict("run revision changed".into()));
		}
		let mut state = persistence::state(&mut access, &run).await?;
		state["revoked"] = json!(true);
		state["cache_bytes"] = json!(0);
		persistence::save_state(&mut access, &run, &state).await?;
		super::maintenance::redact(&mut access.tx, id).await?;
		Ok(json!({"run_id":id,"revoked":true}))
	}
	.await;
	Ok(Json(access.finish(result).await?))
}

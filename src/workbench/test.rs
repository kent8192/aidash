//! Model-backed sandbox. Tool calls are recorded and resolved exclusively from
//! explicit fixtures; no runtime tool executor is reachable from this module.
use super::*;
use crate::{
	provider::{ModelRequest, provider},
	registry::{EntityRef, ModelConfig},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixtureStatus {
	Success,
	Failure,
	Denied,
	Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
	pub status: FixtureStatus,
	pub response: Value,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
	pub expected_revision: i64,
	pub message: String,
	#[serde(default = "simulated_mode")]
	pub mode: String,
	pub profile_id: Option<String>,
	pub continue_from: Option<Uuid>,
	#[serde(default)]
	pub fixtures: BTreeMap<String, Fixture>,
}

fn simulated_mode() -> String {
	"simulated".into()
}

#[derive(Clone)]
struct ProfilePin {
	id: String,
	revision: i64,
	tenant: String,
	rules: Vec<profile::RealToolRule>,
	credential_fingerprints: BTreeMap<String, Vec<u8>>,
}

struct TestJob {
	input: TestInput,
	actor: Actor,
	profile: Option<ProfilePin>,
	tool_references: Vec<EntityRef>,
	limits: TestLimits,
	context_window: usize,
	model_provider: std::sync::Arc<dyn crate::provider::ModelProvider>,
	request: ModelRequest,
	initial_conversation: Vec<Value>,
	pinned_draft: Draft,
	model_credential: Option<(String, Vec<u8>)>,
}

fn credential_fingerprint(name: &str) -> Result<Vec<u8>> {
	Ok(Sha256::digest(crate::config::secret(name)?.as_bytes()).to_vec())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TestSession {
	pub id: Uuid,
	pub draft_id: Uuid,
	pub tenant: String,
	pub revision: i64,
	pub status: String,
	pub scenario: Value,
	pub conversation: Option<Value>,
	pub tool_calls: Option<Value>,
	pub usage: Value,
	pub error: Option<String>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TestLimits {
	pub tenant: String,
	pub max_input_bytes: i32,
	pub max_output_tokens: i32,
	pub max_total_tokens: i32,
	pub max_steps: i32,
	pub max_duration_secs: i32,
	pub max_concurrent: i32,
	pub payload_days: i32,
	pub incident_evidence_days: i32,
}

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(start))
		.routes(routes!(sessions))
		.routes(routes!(stop))
		.routes(routes!(get_limits))
		.routes(routes!(set_limits))
}

const SESSION_COLUMNS: &str = "id, draft_id, tenant, revision, status, scenario, conversation, tool_calls, usage, error, created_at, updated_at, expires_at, expired_at";
const LIMIT_COLUMNS: &str = "tenant, max_input_bytes, max_output_tokens, max_total_tokens, max_steps, max_duration_secs, max_concurrent, payload_days, incident_evidence_days";

async fn load_session(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<TestSession> {
	sqlx::query_as(
		&Query::select()
			.expr(Expr::cust(SESSION_COLUMNS))
			.from(Alias::new("agent_test_sessions"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or_else(|| Error::NotFound("test session".into()))
}

pub(super) async fn limits(tx: &mut Transaction<'_, Postgres>, tenant: &str) -> Result<TestLimits> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_test_limits"))
			.columns([Alias::new("tenant")])
			.values_panic([Expr::cust("$1")])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.execute(&mut **tx)
	.await?;
	sqlx::query_as(
		&Query::select()
			.expr(Expr::cust(LIMIT_COLUMNS))
			.from(Alias::new("agent_test_limits"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.fetch_one(&mut **tx)
	.await
	.map_err(Into::into)
}

fn validate_limits(value: &TestLimits) -> Result<()> {
	if !(1024..=1_000_000).contains(&value.max_input_bytes)
		|| !(64..=8192).contains(&value.max_output_tokens)
		|| !(256..=1_000_000).contains(&value.max_total_tokens)
		|| value.max_total_tokens < value.max_output_tokens
		|| !(1..=64).contains(&value.max_steps)
		|| !(5..=900).contains(&value.max_duration_secs)
		|| !(1..=32).contains(&value.max_concurrent)
		|| !(1..=365).contains(&value.payload_days)
		|| !(1..=3650).contains(&value.incident_evidence_days)
	{
		return Err(Error::Invalid(
			"test limits are outside the supported ranges".into(),
		));
	}
	Ok(())
}

#[utoipa::path(get,path="/workbench/drafts/{id}/test-limits",operation_id="workbench_get_test_limits",params(("id"=Uuid,Path)),responses((status=200,body=TestLimits)),security(("bearer_auth"=[])))]
async fn get_limits(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<TestLimits>> {
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
	let value = limits(&mut tx, &draft.tenant).await?;
	tx.commit().await?;
	Ok(Json(value))
}

#[utoipa::path(put,path="/workbench/test-limits/{tenant}",operation_id="workbench_set_test_limits",params(("tenant"=String,Path)),request_body=TestLimits,responses((status=200,body=TestLimits)),security(("bearer_auth"=[])))]
async fn set_limits(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(tenant): Path<String>,
	Json(input): Json<TestLimits>,
) -> Result<Json<TestLimits>> {
	if !matches!(actor, Actor::Operator) {
		return Err(Error::Forbidden);
	}
	if tenant != input.tenant {
		return Err(Error::Invalid("tenant mismatch".into()));
	}
	validate_limits(&input)?;
	let mut tx = f.store.pool.begin().await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_test_limits"))
			.columns([
				Alias::new("tenant"),
				Alias::new("max_input_bytes"),
				Alias::new("max_output_tokens"),
				Alias::new("max_total_tokens"),
				Alias::new("max_steps"),
				Alias::new("max_duration_secs"),
				Alias::new("max_concurrent"),
				Alias::new("payload_days"),
				Alias::new("incident_evidence_days"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
				Expr::cust("$8"),
				Expr::cust("$9"),
			])
			.on_conflict(
				OnConflict::column(Alias::new("tenant"))
					.update_columns([
						Alias::new("max_input_bytes"),
						Alias::new("max_output_tokens"),
						Alias::new("max_total_tokens"),
						Alias::new("max_steps"),
						Alias::new("max_duration_secs"),
						Alias::new("max_concurrent"),
						Alias::new("payload_days"),
						Alias::new("incident_evidence_days"),
					])
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.bind(input.max_input_bytes)
	.bind(input.max_output_tokens)
	.bind(input.max_total_tokens)
	.bind(input.max_steps)
	.bind(input.max_duration_secs)
	.bind(input.max_concurrent)
	.bind(input.payload_days)
	.bind(input.incident_evidence_days)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(Json(input))
}

/// Remove ordinary test payloads. The metadata and expiry marker remain.
pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("status"), Expr::value("outcome_unknown"))
			.value(Alias::new("active_slot"), Expr::cust("NULL"))
			.value(
				Alias::new("error"),
				Expr::value("test worker stopped before recording a final outcome"),
			)
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("status")).eq("running"))
					.add(Expr::cust(
						"created_at < clock_timestamp() - interval '930 seconds'",
					)),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await?;
	let result = sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("scenario"), Expr::cust("'{}'::jsonb"))
			.value(Alias::new("conversation"), Expr::cust("NULL"))
			.value(Alias::new("tool_calls"), Expr::cust("NULL"))
			.value(Alias::new("expired_at"), Expr::current_timestamp())
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("expires_at")).lte(Expr::current_timestamp()))
					.add(Expr::col(Alias::new("expired_at")).is_null()),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool)
	.await?;
	Ok(result.rows_affected())
}

#[utoipa::path(get,path="/workbench/drafts/{id}/tests",operation_id="workbench_test_sessions",params(("id"=Uuid,Path)),responses((status=200,body=[TestSession])),security(("bearer_auth"=[])))]
async fn sessions(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<Vec<TestSession>>> {
	purge_expired(&f.store.pool).await?;
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
	let rows = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust(SESSION_COLUMNS))
			.from(Alias::new("agent_test_sessions"))
			.and_where(Expr::col(Alias::new("draft_id")).eq(Expr::cust("$1")))
			.order_by(Alias::new("created_at"), Order::Desc)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_all(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(Json(rows))
}

#[utoipa::path(post,path="/workbench/tests/{id}/stop",operation_id="workbench_stop_test",params(("id"=Uuid,Path)),responses((status=200,body=TestSession)),security(("bearer_auth"=[])))]
async fn stop(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
) -> Result<Json<TestSession>> {
	let mut tx = f.store.pool.begin().await?;
	let session = load_session(&mut tx, id).await?;
	let draft = load(&mut tx, session.draft_id, false).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("status"), Expr::value("stopped"))
			.value(Alias::new("active_slot"), Expr::cust("NULL"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running")),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.execute(&mut *tx)
	.await?;
	let result = load_session(&mut tx, id).await?;
	tx.commit().await?;
	Ok(Json(result))
}

#[utoipa::path(post,path="/workbench/drafts/{id}/tests",operation_id="workbench_start_test",params(("id"=Uuid,Path)),request_body=TestInput,responses((status=200,body=TestSession)),security(("bearer_auth"=[])))]
async fn start(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(id): Path<Uuid>,
	Json(input): Json<TestInput>,
) -> Result<Json<TestSession>> {
	if input.message.trim().is_empty() || input.fixtures.len() > 64 {
		return Err(Error::Invalid(
			"test requires a message and at most 64 fixtures".into(),
		));
	}
	if !matches!(input.mode.as_str(), "simulated" | "real") {
		return Err(Error::Invalid("test mode must be simulated or real".into()));
	}
	if input.mode == "simulated" && input.profile_id.is_some() {
		return Err(Error::Invalid(
			"simulated tests cannot select a real-tool profile".into(),
		));
	}
	let mut tx = f.store.pool.begin().await?;
	let draft = load(&mut tx, id, true).await?;
	authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
	if draft.archived || draft.revision != input.expected_revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	let entry = validate_content(&f, &draft, &actor, &mut tx).await?;
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let model = f
		.registry
		.get(&config.model.id, &config.model.version)
		.await?;
	let model_config: ModelConfig = serde_json::from_value(model.config)?;
	let model_credential = model_config
		.credential_env
		.as_ref()
		.map(|name| credential_fingerprint(name).map(|digest| (name.clone(), digest)))
		.transpose()?;
	let limits = limits(&mut tx, &draft.tenant).await?;
	validate_limits(&limits)?;
	let profile = if input.mode == "real" {
		let id = input.profile_id.as_deref().ok_or_else(|| {
			Error::Invalid("real tests require an administrator-configured profile".into())
		})?;
		let value = profile::load(&mut tx, &draft.tenant, id).await?;
		if !value.enabled {
			return Err(Error::Forbidden);
		}
		let rules: Vec<profile::RealToolRule> = serde_json::from_value(value.rules)?;
		if rules.is_empty() {
			return Err(Error::Invalid(
				"real-tool profile has no permitted Tools".into(),
			));
		}
		for rule in &rules {
			if !config.tools.contains(&rule.tool) {
				return Err(Error::Forbidden);
			}
		}
		let mut credential_fingerprints = BTreeMap::new();
		for name in rules.iter().filter_map(|rule| rule.credential_env.as_ref()) {
			credential_fingerprints.insert(name.clone(), credential_fingerprint(name)?);
		}
		Some(ProfilePin {
			id: value.id,
			revision: value.revision,
			tenant: draft.tenant.clone(),
			rules,
			credential_fingerprints,
		})
	} else {
		None
	};
	let mut conversation = if let Some(previous_id) = input.continue_from {
		let previous = load_session(&mut tx, previous_id).await?;
		if previous.draft_id != draft.id
			|| previous.revision != draft.revision
			|| previous.status != "completed"
			|| previous.expired_at.is_some()
			|| previous.scenario["mode"] != input.mode
			|| previous.scenario["profile_id"] != serde_json::to_value(&input.profile_id)?
			|| previous.scenario["profile_revision"]
				!= serde_json::to_value(profile.as_ref().map(|pin| pin.revision))?
		{
			return Err(Error::Conflict(
				"previous test context is stale or unavailable; reset the conversation".into(),
			));
		}
		previous
			.conversation
			.and_then(|value| value.as_array().cloned())
			.ok_or_else(|| Error::Conflict("previous test conversation is unavailable".into()))?
	} else {
		Vec::new()
	};
	conversation.push(json!({"role":"user","content":input.message}));
	let tool_references = config.tools.clone();
	let mut instructions = crate::context::agent_instructions("");
	if input.mode == "real" {
		instructions.push_str("\n\nSandbox: only tools in the selected test connection profile can reach its isolated endpoint. Other tools need an explicit fixture; never claim an unprovided result.\n");
	} else {
		instructions.push_str("\n\nSandbox: all tool calls are simulated from explicit fixtures. Never claim an unprovided tool result.\n");
	}
	for skill in &config.skills {
		let skill = f.registry.get(&skill.id, &skill.version).await?;
		instructions.push_str("\nSkill:\n");
		instructions.push_str(&crate::registry::skill_instructions(&skill)?);
	}
	instructions.push_str("\nAdditional instructions:\n");
	instructions.push_str(&config.instructions);
	let mut tool_specs = crate::tool::builtins()
		.into_iter()
		.filter(|(name, _)| config.permits_builtin(name))
		.map(|(_, tool)| tool.specification())
		.collect::<Vec<_>>();
	for (index, reference) in config.tools.iter().enumerate() {
		let tool = f.registry.get(&reference.id, &reference.version).await?;
		if config.allow_task_delegation == Some(false)
			&& matches!(
				serde_json::from_value::<crate::tool::ToolConfig>(tool.config.clone())?,
				crate::tool::ToolConfig::Agent { .. }
			) {
			continue;
		}
		tool_specs.push(crate::tool::plugin_specification(
			&tool,
			&format!("plugin_{index}"),
		));
	}
	let mut request = ModelRequest {
		instructions,
		context: json!({"test_message":input.message,"private_references":draft.documents,"test_mode":input.mode,"profile_id":input.profile_id}),
		tools: tool_specs,
		max_output_tokens: (limits.max_output_tokens as u32).min(model_config.output_token_limit()),
	};
	if input.continue_from.is_some() {
		request.context["conversation"] = json!(conversation);
	}
	let serialized_bytes = serde_json::to_vec(&request)?.len();
	if serialized_bytes > limits.max_input_bytes as usize
		|| request.estimated_total_tokens() > model_config.context_window
		|| request.estimated_total_tokens() > limits.max_total_tokens as usize
	{
		return Err(Error::Invalid(
			"test input exceeds configured or model context limits".into(),
		));
	}
	let context_window = model_config.context_window;
	let model_provider = provider(f.client.clone(), model_config)?;
	let fixtures = serde_json::to_value(&input.fixtures)?;
	if fixtures.to_string().len() > 65_536 {
		return Err(Error::Invalid("test fixtures exceed 64 KiB".into()));
	}
	// A dead server or canceled request cannot hold the admission slot forever.
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("status"), Expr::value("outcome_unknown"))
			.value(Alias::new("active_slot"), Expr::cust("NULL"))
			.value(
				Alias::new("error"),
				Expr::value("test worker stopped before recording a final outcome"),
			)
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running"))
					.add(Expr::cust(
						"created_at < clock_timestamp() - make_interval(secs => $2)",
					)),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&draft.tenant)
	.bind(limits.max_duration_secs + 30)
	.execute(&mut *tx)
	.await?;
	let active: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("count(*)"))
			.from(Alias::new("agent_test_sessions"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running")),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&draft.tenant)
	.fetch_one(&mut *tx)
	.await?;
	if active >= i64::from(limits.max_concurrent) {
		return Err(Error::Conflict(
			"tenant test concurrency limit reached".into(),
		));
	}
	let session_id = Uuid::now_v7();
	let expires = Utc::now() + chrono::Duration::days(i64::from(limits.payload_days));
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_test_sessions"))
			.columns([
				Alias::new("id"),
				Alias::new("draft_id"),
				Alias::new("tenant"),
				Alias::new("revision"),
				Alias::new("status"),
				Alias::new("scenario"),
				Alias::new("conversation"),
				Alias::new("tool_calls"),
				Alias::new("usage"),
				Alias::new("active_slot"),
				Alias::new("expires_at"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::value("running"),
				Expr::cust("$5"),
				Expr::cust("$6"),
				Expr::cust("$7"),
				Expr::cust("$8"),
				Expr::cust("$9"),
				Expr::cust("$10"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(session_id)
	.bind(id)
	.bind(&draft.tenant)
	.bind(draft.revision)
	.bind(json!({"mode":input.mode,"profile_id":input.profile_id,"profile_revision":profile.as_ref().map(|value|value.revision),"continue_from":input.continue_from,"fixtures":fixtures}))
	.bind(json!(conversation))
	.bind(json!([]))
	.bind(json!({}))
	.bind(id)
	.bind(expires)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	let mut tx = f.store.pool.begin().await?;
	let session = load_session(&mut tx, session_id).await?;
	tx.commit().await?;
	tokio::spawn(async move {
		if let Err(error) = complete(
			f,
			session_id,
			TestJob {
				input,
				actor,
				profile,
				tool_references,
				limits,
				context_window,
				model_provider,
				request,
				initial_conversation: conversation,
				pinned_draft: draft,
				model_credential,
			},
		)
		.await
		{
			tracing::error!(%session_id, %error, "sandbox session completion failed");
		}
	});
	Ok(Json(session))
}

async fn complete(f: Federation, session_id: Uuid, job: TestJob) -> Result<()> {
	let outcome = tokio::time::timeout(
		std::time::Duration::from_secs(job.limits.max_duration_secs as u64),
		simulate(&f, session_id, &job),
	)
	.await;
	let (status, conversation, calls, usage, error) = match outcome {
		Ok(Ok(result)) => result,
		Ok(Err(error)) => {
			let mut tx = f.store.pool.begin().await?;
			let prior = load_session(&mut tx, session_id).await?;
			tx.commit().await?;
			(
				if has_unknown_call(&prior.tool_calls) {
					"outcome_unknown"
				} else {
					"failed"
				},
				prior
					.conversation
					.unwrap_or_else(|| json!([{"role":"user","content":job.input.message}])),
				prior.tool_calls.unwrap_or_else(|| json!([])),
				prior.usage,
				Some(error.to_string()),
			)
		}
		Err(_) => {
			let mut tx = f.store.pool.begin().await?;
			let prior = load_session(&mut tx, session_id).await?;
			tx.commit().await?;
			let dispatched = has_unknown_call(&prior.tool_calls);
			(
				if dispatched {
					"outcome_unknown"
				} else {
					"timed_out"
				},
				prior
					.conversation
					.unwrap_or_else(|| json!([{"role":"user","content":job.input.message}])),
				prior.tool_calls.unwrap_or_else(|| json!([])),
				prior.usage,
				Some(if dispatched {
					"test timed out while an external call was in flight; its outcome is unknown"
						.into()
				} else {
					"model request timed out; provider outcome is unknown".into()
				}),
			)
		}
	};
	let mut tx = f.store.pool.begin().await?;
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("status"), Expr::cust("$2"))
			.value(Alias::new("conversation"), Expr::cust("$3"))
			.value(Alias::new("tool_calls"), Expr::cust("$4"))
			.value(Alias::new("usage"), Expr::cust("$5"))
			.value(Alias::new("error"), Expr::cust("$6"))
			.value(Alias::new("active_slot"), Expr::cust("NULL"))
			.value(Alias::new("updated_at"), Expr::current_timestamp())
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running")),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(session_id)
	.bind(status)
	.bind(conversation)
	.bind(calls)
	.bind(usage)
	.bind(error)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(())
}

type SimulationResult = (&'static str, Value, Value, Value, Option<String>);

fn has_unknown_call(calls: &Option<Value>) -> bool {
	calls
		.as_ref()
		.and_then(Value::as_array)
		.is_some_and(|items| {
			items
				.iter()
				.any(|item| item["outcome"] == "outcome_unknown")
		})
}

async fn invoke_real(
	f: &Federation,
	session_id: Uuid,
	actor: &Actor,
	pin: &ProfilePin,
	rule: &profile::RealToolRule,
	call: &crate::provider::ToolCall,
) -> Result<(Value, &'static str)> {
	let action = call.arguments["action"]
		.as_str()
		.ok_or_else(|| Error::Invalid("real Tool arguments require an action".into()))?;
	let resource = call.arguments["resource"]
		.as_str()
		.ok_or_else(|| Error::Invalid("real Tool arguments require a resource".into()))?;
	if !rule.allowed_actions.iter().any(|allowed| allowed == action)
		|| !rule
			.allowed_resources
			.iter()
			.any(|allowed| allowed == resource)
	{
		return Err(Error::Forbidden);
	}
	let tool = f.registry.get(&rule.tool.id, &rule.tool.version).await?;
	jsonschema::validator_for(&tool.schema)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.validate(&call.arguments)
		.map_err(|e| Error::Invalid(e.to_string()))?;
	// Commit the exact pending call before network I/O. A crash after dispatch
	// leaves outcome_unknown evidence; the worker never retries that call.
	let pending = json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"outcome_unknown","endpoint":rule.endpoint});
	let mut tx = f.store.pool.begin().await?;
	let session = load_session(&mut tx, session_id).await?;
	if session.status != "running" {
		return Err(Error::Conflict("test was stopped".into()));
	}
	let mut calls = session.tool_calls.unwrap_or_else(|| json!([]));
	calls
		.as_array_mut()
		.ok_or_else(|| Error::Invalid("invalid test call log".into()))?
		.push(pending);
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("tool_calls"), Expr::cust("$2"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running")),
			)
			.to_string(PostgresQueryBuilder),
	)
	.bind(session_id)
	.bind(calls)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;

	// Hold the exact session, identity, policy, draft and profile authority
	// through dispatch. Stop/revocation/profile changes wait for this boundary.
	let mut tx = f.store.pool.begin().await?;
	let active: Option<String> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("status"))
			.from(Alias::new("agent_test_sessions"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(session_id)
	.fetch_optional(&mut *tx)
	.await?;
	if active.as_deref() != Some("running") {
		return Err(Error::Conflict("test was stopped".into()));
	}
	if let Actor::Subject(identity) = actor {
		identity.lock_with_mode(&mut tx, false).await?;
	}
	let draft = load(&mut tx, session.draft_id, false).await?;
	authorize(&mut tx, actor, &draft, "agent_draft.test", true).await?;
	let current = profile::load(&mut tx, &pin.tenant, &pin.id).await?;
	if !current.enabled
		|| current.revision != pin.revision
		|| current.rules != serde_json::to_value(&pin.rules)?
	{
		return Err(Error::Conflict(
			"test profile changed or was disabled".into(),
		));
	}
	for (name, fingerprint) in &pin.credential_fingerprints {
		if credential_fingerprint(name)? != *fingerprint {
			return Err(Error::Conflict("test credential changed".into()));
		}
	}
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(30))
		.build()?;
	let mut request = client
		.post(&rule.endpoint)
		.header("idempotency-key", format!("test-{session_id}-{}", call.id))
		.json(&call.arguments);
	if let Some(name) = &rule.credential_env {
		request = request.bearer_auth(crate::config::secret(name)?);
	}
	let response = request.send().await?;
	let (result, outcome) = if response.status().is_success() {
		(crate::response::json(response, 256_000).await?, "real")
	} else {
		(
			json!({"error":"test Tool returned an HTTP error","status":response.status().as_u16()}),
			"failed",
		)
	};
	let mut recorded = load_session(&mut tx, session_id)
		.await?
		.tool_calls
		.ok_or_else(|| Error::Conflict("pending test Tool call was lost".into()))?;
	let last = recorded
		.as_array_mut()
		.and_then(|calls| calls.last_mut())
		.ok_or_else(|| Error::Conflict("pending test Tool call was lost".into()))?;
	if last["id"] != call.id || last["outcome"] != "outcome_unknown" {
		return Err(Error::Conflict("pending test Tool call changed".into()));
	}
	*last = json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":outcome,"result":result,"endpoint":rule.endpoint});
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_test_sessions"))
			.value(Alias::new("tool_calls"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(session_id)
	.bind(recorded)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok((result, outcome))
}

async fn simulate(f: &Federation, session_id: Uuid, job: &TestJob) -> Result<SimulationResult> {
	let TestJob {
		input,
		actor,
		profile,
		tool_references,
		limits,
		context_window,
		model_provider,
		..
	} = job;
	let mut request = job.request.clone();
	let mut conversation = job.initial_conversation.clone();
	let mut calls = Vec::new();
	let mut input_tokens = 0_u64;
	let mut output_tokens = 0_u64;
	let mut usage_complete = true;
	let mut status = "blocked";
	let mut error = None;
	for _ in 0..limits.max_steps {
		let still_running: bool = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("status = 'running'"))
				.from(Alias::new("agent_test_sessions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(session_id)
		.fetch_one(&f.store.pool)
		.await?;
		if !still_running {
			status = "stopped";
			break;
		}
		if let Some((name, fingerprint)) = &job.model_credential
			&& credential_fingerprint(name)? != *fingerprint
		{
			return Err(Error::Conflict(
				"model credential changed during test".into(),
			));
		}
		let mut authority = f.store.pool.begin().await?;
		if let Actor::Subject(identity) = actor {
			identity.lock_with_mode(&mut authority, false).await?;
		}
		let current_draft = load(&mut authority, job.pinned_draft.id, false).await?;
		authorize(
			&mut authority,
			actor,
			&current_draft,
			"agent_draft.test",
			true,
		)
		.await?;
		validate_content(f, &job.pinned_draft, actor, &mut authority).await?;
		if let Some(pin) = profile {
			let current = profile::load(&mut authority, &pin.tenant, &pin.id).await?;
			if !current.enabled
				|| current.revision != pin.revision
				|| current.rules != serde_json::to_value(&pin.rules)?
			{
				return Err(Error::Conflict(
					"test profile changed during session".into(),
				));
			}
		}
		authority.commit().await?;
		if request.input_body().to_string().len() > limits.max_input_bytes as usize
			|| request.estimated_total_tokens() > *context_window
			|| input_tokens
				.saturating_add(output_tokens)
				.saturating_add(request.estimated_total_tokens() as u64)
				> limits.max_total_tokens as u64
		{
			error = Some("test context exceeds configured input or model window limit".into());
			break;
		}
		let response = model_provider.infer(request.clone()).await?;
		input_tokens = input_tokens.saturating_add(response.input_tokens);
		output_tokens = output_tokens.saturating_add(response.output_tokens);
		usage_complete &= response.usage_complete;
		if output_tokens > limits.max_output_tokens as u64 {
			error = Some("test output token limit exceeded by model response".into());
			break;
		}
		if input_tokens.saturating_add(output_tokens) > limits.max_total_tokens as u64 {
			error = Some("test total token limit exceeded by model response".into());
			break;
		}
		conversation.push(
			json!({"role":"assistant","content":response.text,"tool_calls":response.tool_calls}),
		);
		if response.tool_calls.is_empty() {
			status = "completed";
			break;
		}
		let mut missing = false;
		for call in response.tool_calls {
			if calls.len() >= limits.max_steps as usize {
				error = Some("test step limit reached".into());
				missing = true;
				break;
			}
			let real_rule = profile.as_ref().and_then(|pin| {
				call.name
					.strip_prefix("plugin_")
					.and_then(|index| index.parse::<usize>().ok())
					.and_then(|index| {
						request
							.tools
							.iter()
							.find(|tool| tool.name == call.name)
							.map(|_| index)
					})
					.and_then(|index| tool_references.get(index))
					.and_then(|selected| pin.rules.iter().find(|rule| selected == &rule.tool))
			});
			let fixture = input.fixtures.get(&call.name);
			let result = if let Some(rule) = real_rule {
				match invoke_real(
					f,
					session_id,
					actor,
					profile.as_ref().expect("real rule requires profile"),
					rule,
					&call,
				)
				.await
				{
					Ok((output, outcome)) => {
						json!({"id":call.id,"name":call.name,"arguments":call.arguments,"result":output,"outcome":outcome})
					}
					Err(reason) => {
						missing = true;
						let mut tx = f.store.pool.begin().await?;
						let pending = load_session(&mut tx, session_id).await?.tool_calls;
						tx.commit().await?;
						let dispatched = has_unknown_call(&pending);
						status = if dispatched {
							"outcome_unknown"
						} else {
							"blocked"
						};
						error = Some(reason.to_string());
						json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":if dispatched { "outcome_unknown" } else { "denied" },"error":reason.to_string()})
					}
				}
			} else {
				if fixture.is_none() {
					missing = true;
				}
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"fixture":fixture,"outcome":if fixture.is_none() { "missing_fixture" } else { "simulated" }})
			};
			conversation.push(json!({"role":"tool","content":result}));
			calls.push(result);
			sqlx::query(&Query::update().table(Alias::new("agent_test_sessions"))
				.value(Alias::new("conversation"), Expr::cust("$2"))
				.value(Alias::new("tool_calls"), Expr::cust("$3"))
				.value(Alias::new("usage"), Expr::cust("$4"))
				.value(Alias::new("updated_at"), Expr::current_timestamp())
				.cond_where(Condition::all()
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("status")).eq("running")))
				.to_string(PostgresQueryBuilder))
				.bind(session_id)
				.bind(json!(conversation))
				.bind(json!(calls))
				.bind(json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete}))
				.execute(&f.store.pool).await?;
		}
		if missing {
			if error.is_none() {
				error = Some("a tool call has no explicit simulated fixture".into());
			}
			break;
		}
		if output_tokens >= limits.max_output_tokens as u64 {
			error = Some("test output token limit reached".into());
			break;
		}
		request.max_output_tokens =
			(limits.max_output_tokens as u64 - output_tokens).min(u32::MAX as u64) as u32;
		request.context["conversation"] = json!(conversation);
	}
	if status == "blocked" && error.is_none() {
		error = Some("test step limit reached".into());
	}
	Ok((
		status,
		json!(conversation),
		json!(calls),
		json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete}),
		error,
	))
}

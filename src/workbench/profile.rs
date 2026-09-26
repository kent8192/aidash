//! Operator-owned, tenant-specific connection profiles for confined real-tool tests.
use super::*;
use crate::{config, registry::EntityRef, tool::ToolConfig};
use axum::extract::Query as QueryParams;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RealToolRule {
	pub tool: EntityRef,
	/// A test endpoint, distinct from the immutable production Tool endpoint.
	pub endpoint: String,
	/// Environment variable name only. The secret value is never returned.
	pub credential_env: Option<String>,
	pub allowed_actions: Vec<String>,
	pub allowed_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct TestProfile {
	pub tenant: String,
	pub id: String,
	pub revision: i64,
	pub enabled: bool,
	pub rules: Value,
	pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
	pub expected_revision: i64,
	pub enabled: bool,
	pub rules: Vec<RealToolRule>,
}

#[derive(Debug, Deserialize)]
pub struct ProfileQuery {
	pub tenant: Option<String>,
	pub draft_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileSummary {
	pub id: String,
	pub revision: i64,
	pub enabled: bool,
}

const PROFILE_COLUMNS: &str = "tenant, id, revision, enabled, rules, updated_at";

pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(put))
}

pub async fn load(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	id: &str,
) -> Result<TestProfile> {
	sqlx::query_as(
		&Query::select()
			.expr(Expr::cust(PROFILE_COLUMNS))
			.from(Alias::new("agent_test_profiles"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$2"))),
			)
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.bind(id)
	.fetch_optional(&mut **tx)
	.await?
	.ok_or_else(|| Error::NotFound("test profile".into()))
}

#[utoipa::path(get,path="/workbench/test-profiles",operation_id="workbench_test_profiles",params(("tenant"=Option<String>,Query),("draft_id"=Option<Uuid>,Query)),responses((status=200,body=[ProfileSummary])),security(("bearer_auth"=[])))]
async fn list(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	QueryParams(query): QueryParams<ProfileQuery>,
) -> Result<Json<Vec<ProfileSummary>>> {
	let mut draft_tools = None;
	let tenant = match actor {
		Actor::Operator => query
			.tenant
			.ok_or_else(|| Error::Invalid("tenant is required".into()))?,
		Actor::Subject(subject) => {
			if query
				.tenant
				.as_deref()
				.is_some_and(|value| value != subject.tenant)
			{
				return Err(Error::Forbidden);
			}
			let draft_id = query
				.draft_id
				.ok_or_else(|| Error::Invalid("draft_id is required".into()))?;
			let mut tx = f.store.pool.begin().await?;
			let draft = super::load(&mut tx, draft_id, false).await?;
			super::authorize(
				&mut tx,
				&Actor::Subject(subject.clone()),
				&draft,
				"agent_draft.test",
				true,
			)
			.await?;
			let config: AgentConfig = serde_json::from_value(draft.entry["config"].clone())?;
			draft_tools = Some(config.tools);
			tx.commit().await?;
			subject.tenant
		}
	};
	let rows = sqlx::query_as(
		&Query::select()
			.expr(Expr::cust(PROFILE_COLUMNS))
			.from(Alias::new("agent_test_profiles"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(100)
			.to_string(PostgresQueryBuilder),
	)
	.bind(tenant)
	.fetch_all(&f.store.pool)
	.await?;
	Ok(Json(
		rows.into_iter()
			.filter(|row: &TestProfile| {
				let Some(tools) = &draft_tools else {
					return true;
				};
				if !row.enabled {
					return false;
				}
				serde_json::from_value::<Vec<RealToolRule>>(row.rules.clone()).is_ok_and(|rules| {
					!rules.is_empty() && rules.iter().all(|rule| tools.contains(&rule.tool))
				})
			})
			.map(|row: TestProfile| ProfileSummary {
				id: row.id,
				revision: row.revision,
				enabled: row.enabled,
			})
			.collect(),
	))
}

#[utoipa::path(put,path="/workbench/test-profiles/{tenant}/{id}",operation_id="workbench_put_test_profile",params(("tenant"=String,Path),("id"=String,Path)),request_body=ProfileInput,responses((status=200,body=TestProfile)),security(("bearer_auth"=[])))]
async fn put(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((tenant, id)): Path<(String, String)>,
	Json(input): Json<ProfileInput>,
) -> Result<Json<TestProfile>> {
	if !matches!(actor, Actor::Operator) {
		return Err(Error::Forbidden);
	}
	if tenant.trim().is_empty() || id.trim().is_empty() || id.len() > 100 || input.rules.len() > 32
	{
		return Err(Error::Invalid(
			"invalid test profile identity or size".into(),
		));
	}
	let mut seen = HashSet::new();
	for rule in &input.rules {
		if !seen.insert((&rule.tool.id, &rule.tool.version))
			|| rule.allowed_actions.is_empty()
			|| rule.allowed_resources.is_empty()
			|| rule.allowed_actions.len() > 64
			|| rule.allowed_resources.len() > 64
			|| rule
				.allowed_actions
				.iter()
				.chain(&rule.allowed_resources)
				.any(|value| value.trim().is_empty() || value.len() > 200)
		{
			return Err(Error::Invalid(
				"real-tool rules need unique Tools and bounded action/resource allowlists".into(),
			));
		}
		config::validate_endpoint(&rule.endpoint)?;
		let url = reqwest::Url::parse(&rule.endpoint)
			.map_err(|_| Error::Invalid("invalid test endpoint".into()))?;
		if url.scheme() != "https"
			&& !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
		{
			return Err(Error::Invalid(
				"test endpoints require HTTPS except on loopback".into(),
			));
		}
		if let Some(name) = &rule.credential_env {
			config::secret(name)?;
		}
		let tool = f.registry.get(&rule.tool.id, &rule.tool.version).await?;
		if tool.kind != "tool" {
			return Err(Error::Invalid(
				"test profile references a non-Tool Registry entry".into(),
			));
		}
		let cfg: ToolConfig = serde_json::from_value(tool.config)?;
		match cfg {
			ToolConfig::Http { endpoint, replay, credential_env } if replay == "read_only" && reqwest::Url::parse(&endpoint).map_err(|_| Error::Invalid("invalid production endpoint".into()))? != url && match &credential_env {
				Some(production) => rule.credential_env.as_ref().is_some_and(|test| test != production),
				None => true,
			} => {},
			_ => return Err(Error::Invalid("real tests require an HTTP read-only Tool, a separate test endpoint, and separate test credentials".into())),
		}
	}
	let rules = serde_json::to_value(&input.rules)?;
	let mut tx = f.store.pool.begin().await?;
	let existing: Option<i64> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("revision"))
			.from(Alias::new("agent_test_profiles"))
			.cond_where(
				Condition::all()
					.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
					.add(Expr::col(Alias::new("id")).eq(Expr::cust("$2"))),
			)
			.lock(sea_orm::sea_query::LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&tenant)
	.bind(&id)
	.fetch_optional(&mut *tx)
	.await?;
	if existing.unwrap_or(0) != input.expected_revision {
		return Err(Error::Conflict("test profile changed".into()));
	}
	if existing.is_some() {
		sqlx::query(
			&Query::update()
				.table(Alias::new("agent_test_profiles"))
				.value(Alias::new("revision"), Expr::cust("revision + 1"))
				.value(Alias::new("enabled"), Expr::cust("$3"))
				.value(Alias::new("rules"), Expr::cust("$4"))
				.value(Alias::new("updated_at"), Expr::current_timestamp())
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("id")).eq(Expr::cust("$2"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(&tenant)
		.bind(&id)
		.bind(input.enabled)
		.bind(&rules)
		.execute(&mut *tx)
		.await?;
	} else {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_test_profiles"))
				.columns([
					Alias::new("tenant"),
					Alias::new("id"),
					Alias::new("enabled"),
					Alias::new("rules"),
				])
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
				])
				.to_string(PostgresQueryBuilder),
		)
		.bind(&tenant)
		.bind(&id)
		.bind(input.enabled)
		.bind(&rules)
		.execute(&mut *tx)
		.await?;
	}
	let result = load(&mut tx, &tenant, &id).await?;
	tx.commit().await?;
	Ok(Json(result))
}

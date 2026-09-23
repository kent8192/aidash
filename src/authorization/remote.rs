//! Durable source authority for remote admission. A prepared grant is pinned to
//! a task revision and exact receiver definitions; possession never bypasses
//! current policy, credential, task, peer or receiver checks.
use super::{
	access::Access,
	execution::inherit_task_origin,
	identity::{Actor, SubjectIdentity},
	peer::execution::Inspection,
	policy::SubjectKind,
};
use crate::{
	Error, Result,
	domain::{Task, qualified_agent},
	federation::{Federation, Peer},
	registry::{AgentConfig, EntityRef, Search, digest},
};
use axum::{
	Extension, Json,
	extract::{Path, State},
	http::HeaderMap,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareInput {
	/// Caller-supplied idempotency key. Reuse requires identical authority and definitions.
	id: Uuid,
	node_id: String,
	agent: EntityRef,
	ttl_seconds: i64,
}
#[derive(Clone, sqlx::FromRow)]
struct Grant {
	id: Uuid,
	task_id: Uuid,
	task_revision: i64,
	workspace_id: Uuid,
	node_id: String,
	tenant: String,
	credential_id: Uuid,
	root_subject: String,
	subject_chain: Vec<String>,
	inspection: Value,
	expires_at: DateTime<Utc>,
	revoked: bool,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct Prepared {
	id: Uuid,
	task_id: Uuid,
	node_id: String,
	agent: EntityRef,
	expires_at: DateTime<Utc>,
	revoked: bool,
}
impl Grant {
	fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			credential_id: self.credential_id,
			tenant: self.tenant.clone(),
			subject: self.root_subject.clone(),
		}
	}
	fn prepared(&self) -> Result<Prepared> {
		let inspection: Inspection = serde_json::from_value(self.inspection.clone())?;
		Ok(Prepared {
			id: self.id,
			task_id: self.task_id,
			node_id: self.node_id.clone(),
			agent: EntityRef {
				id: inspection.agent.id,
				version: inspection.agent.version,
			},
			expires_at: self.expires_at,
			revoked: self.revoked,
		})
	}
}
pub fn routes() -> OpenApiRouter<Federation> {
	OpenApiRouter::new()
		.routes(routes!(prepare))
		.routes(routes!(revoke))
}

// A receiver may describe only the requested Agent's exact direct dependencies.
// Never use peer-provided node IDs or arbitrary resource lists as authority.
fn validate(
	inspection: &Inspection,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
) -> Result<()> {
	if inspection.authority_digest.len() != 71
		|| !inspection.authority_digest.starts_with("sha256:")
		|| !inspection.authority_digest.as_bytes()[7..]
			.iter()
			.all(u8::is_ascii_hexdigit)
		|| inspection.node_id != node
		|| inspection.agent.id != agent.id
		|| inspection.agent.version != agent.version
		|| inspection.agent.kind != "agent"
		|| !requirements.matches(&inspection.agent)
	{
		return Err(Error::External(
			"invalid receiver execution inspection".into(),
		));
	}
	crate::registry::validate(&inspection.agent)?;
	let config: AgentConfig = serde_json::from_value(inspection.agent.config.clone())?;
	let mut expected = BTreeMap::new();
	for (reference, kind) in std::iter::once((agent, "agent"))
		.chain(std::iter::once((&config.model, "model")))
		.chain(config.tools.iter().map(|r| (r, "tool")))
		.chain(config.skills.iter().map(|r| (r, "skill")))
		.chain(config.cluster.iter().map(|r| (r, "cluster")))
	{
		if let Some(previous) = expected.insert((&reference.id, &reference.version), kind)
			&& previous != kind
		{
			return Err(Error::External(
				"inconsistent receiver dependency kinds".into(),
			));
		}
	}
	if inspection.definitions.len() != expected.len() {
		return Err(Error::External("incomplete receiver definitions".into()));
	}
	for definition in &inspection.definitions {
		if definition.metadata.id != definition.entry.id
			|| definition.metadata.version != definition.entry.version
			|| definition.metadata.kind != definition.kind
			|| definition.digest != digest(&serde_json::to_value(&definition.metadata)?)
		{
			return Err(Error::External(
				"receiver definition metadata mismatch".into(),
			));
		}
		// Dependencies execute on the receiver. Do not resolve that node's
		// credential environment or executable paths on this source node.
		super::policy::identifier(&definition.entry.id)?;
		semver::Version::parse(&definition.entry.version)
			.map_err(|_| Error::External("invalid receiver version".into()))?;
		if expected.remove(&(&definition.entry.id, &definition.entry.version))
			!= Some(definition.kind.as_str())
			|| definition.digest.len() != 71
			|| !definition.digest.starts_with("sha256:")
			|| !definition.digest[7..]
				.bytes()
				.all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::External("invalid receiver definition".into()));
		}
		if definition.kind == "agent"
			&& definition.digest != digest(&serde_json::to_value(&inspection.agent)?)
		{
			return Err(Error::External("receiver agent digest mismatch".into()));
		}
	}
	Ok(())
}

async fn source_authority(
	access: &mut Access,
	task: &Task,
	node: &str,
	inspection: &Inspection,
) -> Result<()> {
	let executor = qualified_agent(node, &inspection.agent.id, &inspection.agent.version);
	if access.subjects.last() != Some(&executor)
		|| access
			.snapshot
			.bundle
			.subjects
			.get(&executor)
			.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	let workspace = access.workspace(task.workspace_id).await?;
	access.context = workspace.attributes.clone();
	access.require(&workspace, "workspace.read").await?;
	let resource = access.task_resource(task).await?;
	access.require(&resource, "task.read").await?;
	access.require(&resource, "task.delegate").await?;
	access.require(&resource, "task.execute").await?;
	let resource = access.resource("node", node, json!({"remote_node":node}));
	access.require(&resource, "federation.execute").await?;
	for definition in &inspection.definitions {
		let id = format!(
			"{node}/{}s/{}@{}",
			definition.kind, definition.entry.id, definition.entry.version
		);
		let mut resource = super::catalog::resource(access, &definition.metadata);
		resource.id = id;
		resource.attributes["remote_node"] = json!(node);
		resource.attributes["digest"] = json!(definition.digest);
		access.require(&resource, "registry.read").await?;
		let action = match definition.kind.as_str() {
			"agent" => "agent.execute",
			"model" => "model.infer",
			"tool" => "tool.invoke",
			"skill" => "skill.use",
			"cluster" => "cluster.execute",
			_ => return Err(Error::Forbidden),
		};
		access.require(&resource, action).await?;
	}
	Ok(())
}
async fn live(access: &mut Access, id: Uuid) -> Result<bool> {
	Ok(sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"NOT revoked AND expires_at > CLOCK_TIMESTAMP()",
			))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_remote_grants",
			))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.fetch_one(&mut **access.tx)
	.await?)
}
async fn peer(access: &mut Access, node: &str) -> Result<()> {
	let peer: Option<Peer> = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new("peers"))
			.and_where(sea_orm::sea_query::Expr::cust("node_id = $1 AND enabled"))
			.lock(sea_orm::sea_query::LockType::Share)
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(node)
	.fetch_optional(&mut **access.tx)
	.await?;
	if peer.is_none_or(|p| p.protocol_version != crate::config::PROTOCOL_VERSION) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
async fn inspect(
	f: &Federation,
	access: &mut Access,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
) -> Result<Inspection> {
	let resource = access.resource("node", node, json!({"remote_node":node}));
	access.require(&resource, "federation.execute").await?;
	peer(access, node).await?;
	let inspection: Inspection = super::peer::authority_request(f, node, "/scoped/execution/inspect", &json!({"tenant":access.identity.tenant,"subject":access.identity.subject,"agent":agent,"requirements":requirements})).await?;
	validate(&inspection, node, agent, requirements)?;
	Ok(inspection)
}
#[utoipa::path(post,path="/tasks/{id}/remote-grants",operation_id="remote_grant_prepare",params(("id"=Uuid,Path)),request_body=PrepareInput,responses((status=200,body=Prepared)),security(("bearer_auth"=[])))]
async fn prepare(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path(task_id): Path<Uuid>,
	Json(input): Json<PrepareInput>,
) -> Result<Json<Prepared>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	if input.node_id == f.config.node_id || !(1..=3600).contains(&input.ttl_seconds) {
		return Err(Error::Invalid(
			"invalid remote grant destination or lifetime".into(),
		));
	}
	crate::config::validate_node_id(&input.node_id)?;
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
        inherit_task_origin(&mut access,task_id).await?;
        let task = access.task_read(task_id).await?;
        if task.status!="OPEN" {return Err(Error::Conflict("task is already assigned".into()));}
        if access.subjects.len()>=32 {return Err(Error::Invalid("execution delegation depth exceeds 32".into()));}
        let executor = qualified_agent(&input.node_id,&input.agent.id,&input.agent.version);
        if access.snapshot.bundle.subjects.get(&executor).is_none_or(|s| s.kind!=SubjectKind::Agent) {return Err(Error::Forbidden);}
        access.subjects.push(executor);
        let workspace = access.workspace(task.workspace_id).await?;
        access.context=workspace.attributes.clone();
        let resource=access.task_resource(&task).await?;
        access.require(&resource,"task.delegate").await?;
        access.require(&resource,"task.execute").await?;
        let requirements: Search = serde_json::from_value(task.requirements.clone())?;
        let inspection=inspect(&f,&mut access,&input.node_id,&input.agent,&requirements).await?;
        source_authority(&mut access,&task,&input.node_id,&inspection).await?;
        let metadata=serde_json::to_value(&inspection)?;
        // Retain the task revision through persistence, after read authorization.
        let current: Task=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))).from(sea_orm::sea_query::Alias::new("tasks")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).lock(sea_orm::sea_query::LockType::Share).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(task_id).fetch_one(&mut **access.tx).await?;
        if current.revision!=task.revision || current.status!="OPEN" {return Err(Error::Conflict("task changed during grant preparation".into()));}
        let inserted=sqlx::query(&sea_orm::sea_query::Query::insert().into_table(sea_orm::sea_query::Alias::new("authorization_remote_grants")).columns([sea_orm::sea_query::Alias::new("id"), sea_orm::sea_query::Alias::new("task_id"), sea_orm::sea_query::Alias::new("task_revision"), sea_orm::sea_query::Alias::new("workspace_id"), sea_orm::sea_query::Alias::new("node_id"), sea_orm::sea_query::Alias::new("tenant"), sea_orm::sea_query::Alias::new("credential_id"), sea_orm::sea_query::Alias::new("root_subject"), sea_orm::sea_query::Alias::new("subject_chain"), sea_orm::sea_query::Alias::new("inspection"), sea_orm::sea_query::Alias::new("expires_at")]).values_panic([sea_orm::sea_query::Expr::cust("$1"), sea_orm::sea_query::Expr::cust("$2"), sea_orm::sea_query::Expr::cust("$3"), sea_orm::sea_query::Expr::cust("$4"), sea_orm::sea_query::Expr::cust("$5"), sea_orm::sea_query::Expr::cust("$6"), sea_orm::sea_query::Expr::cust("$7"), sea_orm::sea_query::Expr::cust("$8"), sea_orm::sea_query::Expr::cust("$9"), sea_orm::sea_query::Expr::cust("$10"), sea_orm::sea_query::Expr::cust("CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => $11)")]).on_conflict(sea_orm::sea_query::OnConflict::new().do_nothing().to_owned()).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(input.id).bind(task.id).bind(task.revision).bind(task.workspace_id).bind(&input.node_id).bind(&identity.tenant).bind(identity.credential_id).bind(&identity.subject).bind(&access.subjects).bind(&metadata).bind(input.ttl_seconds as f64).execute(&mut **access.tx).await?.rows_affected();
        let grant: Grant=sqlx::query_as(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))).from(sea_orm::sea_query::Alias::new("authorization_remote_grants")).and_where(sea_orm::sea_query::Expr::cust("id = $1")).lock(sea_orm::sea_query::LockType::Share).to_string(sea_orm::sea_query::PostgresQueryBuilder)).bind(input.id).fetch_one(&mut **access.tx).await?;
        if grant.task_id != task_id
            || grant.task_revision != task.revision
            || grant.workspace_id != task.workspace_id
            || grant.node_id != input.node_id
            || grant.credential_id != identity.credential_id
            || grant.tenant != identity.tenant
            || grant.root_subject != identity.subject
            || grant.subject_chain != access.subjects
            || grant.inspection != metadata
            || !live(&mut access, grant.id).await?
        {
            return Err(Error::Conflict("grant id already binds different or expired authority".into()));
        }
        if inserted==1 { f.store.event(&mut access.tx,Some(task.workspace_id),"task.remote_grant_prepared",json!({"grant_id":grant.id,"task_id":task_id,"node_id":grant.node_id,"expires_at":grant.expires_at})).await?; }
        Ok(Json(grant.prepared()?))
    }.await;
	access.finish(result).await
}

#[utoipa::path(post,path="/tasks/{id}/remote-grants/{grant}/revoke",operation_id="remote_grant_revoke",params(("id"=Uuid,Path),("grant"=Uuid,Path)),responses((status=200,body=Prepared)),security(("bearer_auth"=[])))]
async fn revoke(
	State(f): State<Federation>,
	Extension(actor): Extension<Actor>,
	Path((task_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Prepared>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result = async {
		let task = access.task_read(task_id).await?;
		let resource = access.task_resource(&task).await?;
		access.require(&resource, "task.delegate").await?;
		let mut grant: Grant = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_remote_grants",
				))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND task_id = $2 AND tenant = $3 AND root_subject = $4",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(task_id)
		.bind(&identity.tenant)
		.bind(&identity.subject)
		.fetch_optional(&mut **access.tx)
		.await?
		.ok_or(Error::Forbidden)?;
		if !grant.revoked {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new(
						"authorization_remote_grants",
					))
					.value(
						sea_orm::sea_query::Alias::new("revoked"),
						sea_orm::sea_query::Expr::cust("TRUE"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.execute(&mut **access.tx)
			.await?;
			f.store
				.event(
					&mut access.tx,
					Some(task.workspace_id),
					"task.remote_grant_revoked",
					json!({"grant_id":id,"task_id":task_id}),
				)
				.await?;
			grant.revoked = true;
		}
		Ok(Json(grant.prepared()?))
	}
	.await;
	access.finish(result).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyInput {
	grant_id: Uuid,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Description {
	pub grant_id: Uuid,
	pub source_node: String,
	pub target_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub task: Task,
	pub inspection: Inspection,
	pub expires_at: DateTime<Utc>,
}
// Only the destination peer may obtain the source-authorized task. Both this
// description and boolean verification share the exact live authority checks.
pub(crate) async fn describe(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<VerifyInput>,
) -> Result<Json<Description>> {
	let (access, description) =
		description_lease(&f, crate::api::peer_node(&headers)?, input.grant_id).await?;
	access.finish(Ok(Json(description))).await
}
pub(crate) async fn verify(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<VerifyInput>,
) -> Result<Json<bool>> {
	let (access, _) =
		description_lease(&f, crate::api::peer_node(&headers)?, input.grant_id).await?;
	access.finish(Ok(Json(true))).await
}
pub(crate) async fn snapshot(
	State(f): State<Federation>,
	headers: HeaderMap,
	Json(input): Json<VerifyInput>,
) -> Result<Json<crate::domain::WorkspaceSnapshot>> {
	let (mut access, description) =
		description_lease(&f, crate::api::peer_node(&headers)?, input.grant_id).await?;
	access.read_grant = Some(description.grant_id);
	let result = async {
		let snapshot = access
			.workspace_snapshot(description.task.workspace_id)
			.await?;
		if !live(&mut access, description.grant_id).await? {
			return Err(Error::Forbidden);
		}
		Ok(Json(snapshot))
	}
	.await;
	access.finish(result).await
}
async fn description_lease(f: &Federation, node: &str, id: Uuid) -> Result<(Access, Description)> {
	let grant: Grant = sqlx::query_as(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::SimpleExpr::from(
				sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
			))
			.from(sea_orm::sea_query::Alias::new(
				"authorization_remote_grants",
			))
			.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND node_id = $2"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(id)
	.bind(node)
	.fetch_optional(&f.store.pool)
	.await?
	.ok_or(Error::Forbidden)?;
	let mut access = Access::begin(&f.store, &grant.identity())
		.await
		.map_err(|e| {
			if matches!(e, Error::Unauthorized) {
				Error::Forbidden
			} else {
				e
			}
		})?;
	let result = async {
		let current: Grant = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new(
					"authorization_remote_grants",
				))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Share)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(grant.id)
		.fetch_one(&mut **access.tx)
		.await?;
		if !live(&mut access, current.id).await? {
			return Err(Error::Forbidden);
		}
		access.subjects = current.subject_chain.clone();
		let task = access.task_read(current.task_id).await?;
		let locked: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Share)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task.id)
		.fetch_one(&mut **access.tx)
		.await?;
		if locked.revision != task.revision {
			return Err(Error::Forbidden);
		}
		if task.revision != current.task_revision
			|| task.workspace_id != current.workspace_id
			|| task.status != "OPEN"
		{
			return Err(Error::Forbidden);
		}
		let inspection: Inspection = serde_json::from_value(current.inspection)?;
		source_authority(&mut access, &task, node, &inspection).await?;
		if !access.grant_reads_visible(current.id).await? {
			return Err(Error::Forbidden);
		}

		let agent = EntityRef {
			id: inspection.agent.id.clone(),
			version: inspection.agent.version.clone(),
		};
		let requirements = serde_json::from_value(task.requirements.clone())?;
		let fresh = inspect(f, &mut access, node, &agent, &requirements).await?;
		if fresh != inspection {
			return Err(Error::Forbidden);
		}
		if !live(&mut access, current.id).await? {
			return Err(Error::Forbidden);
		}
		Ok(Description {
			grant_id: current.id,
			source_node: f.config.node_id.clone(),
			target_node: current.node_id,
			source_tenant: current.tenant,
			source_subject: current.root_subject,
			task,
			inspection,
			expires_at: current.expires_at,
		})
	}
	.await;
	match result {
		Ok(description) => Ok((access, description)),
		Err(error) => access.finish(Err(error)).await,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn untrusted_inspections_cannot_substitute_or_omit_executor_definitions() {
		let reference = EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		};
		let entry: crate::registry::Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Fixture"},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Fixture"}})).unwrap();
		let model: crate::registry::Entry = serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"Fixture"},"config":{}})).unwrap();
		let mut inspection: Inspection = serde_json::from_value(json!({"node_id":"aidash://host","authority_digest":digest(&json!({})),"agent":entry,"definitions":[{"entry":reference,"kind":"agent","digest":digest(&serde_json::to_value(&entry).unwrap()),"metadata":entry},{"entry":{"id":"model","version":"1.0.0"},"kind":"model","digest":digest(&serde_json::to_value(&model).unwrap()),"metadata":model}]})).unwrap();
		assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_ok());
		assert!(
			validate(
				&inspection,
				"aidash://other",
				&reference,
				&Search::default()
			)
			.is_err()
		);
		let original = inspection.clone();
		inspection.definitions.pop();
		assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
		inspection = original.clone();
		inspection.definitions[1] = inspection.definitions[0].clone();
		assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
		inspection = original.clone();
		inspection
			.agent
			.description
			.insert("en".into(), "Substituted".into());
		assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
		inspection = original;
		inspection.definitions[1].digest = format!("sha256:{}", "é".repeat(32));
		assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	}
}

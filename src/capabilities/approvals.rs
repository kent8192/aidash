//! Durable scopes. Approval never changes a policy or overrides an explicit deny.
use super::{
	contracts::*,
	records::{self, Record},
	sessions,
};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		policy::{Resource, SubjectKind},
	},
	domain::Run,
	store::Store,
};
use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::{Alias, Expr, Order, PostgresQueryBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Outbound {
	pub idempotency_key: Uuid,
	pub url: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
	#[default]
	AllowOnce,
	AllowRun,
	Deny,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecision {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	#[serde(default)]
	pub choice: ApprovalChoice,
	pub targets: Option<Vec<String>>,
	pub expires_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Revoke {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
}
fn resource(access: &Access, origin: &str, run: Uuid) -> Resource {
	access.resource(
		"outbound",
		origin,
		json!({"origin":origin,"run_id":run,"action":"network.get"}),
	)
}
pub(crate) fn permitted_origin(store: &Store, url: &str) -> Result<(reqwest::Url, String)> {
	let parsed =
		reqwest::Url::parse(url).map_err(|_| Error::Invalid("INVALID_OUTBOUND_URL".into()))?;
	if url.len() > 4096
		|| parsed.scheme() != "https"
		|| !parsed.username().is_empty()
		|| parsed.password().is_some()
		|| parsed.fragment().is_some()
		|| parsed.port_or_known_default() != Some(443)
	{
		return Err(Error::Invalid(
			"outbound access requires an HTTPS URL without credentials on port 443".into(),
		));
	}
	let origin = parsed.origin().ascii_serialization();
	if !store.capabilities.0.outbound_origins.contains(&origin) {
		return Err(Error::Conflict("OUTBOUND_OPERATOR_DENIED".into()));
	}
	Ok((parsed, origin))
}
/// Returns true for current ordinary authority; false for an approvable absence
/// of an allow. Explicit denies, unknown identities, and delegated denials fail.
fn ordinary(access: &Access, target: &Resource) -> Result<bool> {
	let mut all = true;
	for subject in &access.subjects {
		let decision =
			access
				.snapshot
				.bundle
				.evaluate(&access.evaluation(subject, target, "network.get"));
		if !decision.allowed && decision.reason != "no_matching_allow" {
			return Err(Error::Forbidden);
		}
		all &= decision.allowed;
	}
	Ok(all)
}
fn eligible(access: &Access, subject: &str, requester: &str, target: &Resource) -> bool {
	let mut evaluation = access.evaluation(subject, target, "capability.approve");
	evaluation.environment["transport"] = json!("api");
	access
		.snapshot
		.bundle
		.subjects
		.get(subject)
		.is_some_and(|s| {
			s.enabled
				&& s.kind == SubjectKind::User
				&& (subject == requester || s.attributes["capability_approver"] == true)
		}) && access.snapshot.bundle.evaluate(&evaluation).allowed
}
fn approver(access: &Access, target: &Resource) -> Option<String> {
	if eligible(
		access,
		&access.identity.subject,
		&access.identity.subject,
		target,
	) {
		return Some(access.identity.subject.clone());
	}
	access
		.snapshot
		.bundle
		.subjects
		.iter()
		.find(|(id, _)| eligible(access, id, &access.identity.subject, target))
		.map(|(id, _)| id.clone())
}
pub(crate) fn visible(access: &Access, record: &Record) -> Result<bool> {
	if record.owner == access.identity.subject {
		return Ok(true);
	}
	if record.data["approver"] != access.identity.subject {
		return Ok(false);
	}
	let run: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	Ok(record.data["targets"].as_array().is_some_and(|targets| {
		!targets.is_empty()
			&& targets.iter().all(|origin| {
				origin.as_str().is_some_and(|origin| {
					eligible(
						access,
						&access.identity.subject,
						&record.owner,
						&resource(access, origin, run),
					)
				})
			})
	}))
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Outbound,
) -> Result<Value> {
	let (_, origin) = permitted_origin(store, &input.url)?;
	let digest = crate::registry::digest(&json!(["outbound_get", run.id, input]));
	if let Some(previous) = sessions::cached(access, input.idempotency_key, &digest).await? {
		let id = serde_json::from_value(previous["operation_id"].clone())?;
		let previous = records::get(access, id, "outbound").await?;
		return view(store, access, run, &previous).await;
	}
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if sessions::status(access, area).await?.active_run_id != Some(run.id)
		|| matches!(run.phase.as_str(), "COMPLETED" | "CANCELLED" | "FAILED")
	{
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let target = resource(access, &origin, run.id);
	access.require(&target, "capability.request").await?;
	let direct = ordinary(access, &target)?;
	let designated = approver(access, &target);
	let grants: Vec<Record> = sqlx::query_as(
		&sessions::select("core_records")
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("kind")).eq("grant"))
			.and_where(Expr::col(Alias::new("state")).eq("active"))
			.and_where(Expr::col(Alias::new("expires_at")).gt(Expr::current_timestamp()))
			.and_where(Expr::cust("data->>'run_id' = $3"))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(64)
			.to_string(PostgresQueryBuilder),
	)
	.bind(&access.identity.tenant)
	.bind(&access.identity.subject)
	.bind(run.id.to_string())
	.fetch_all(&mut **access.tx)
	.await?;
	let grant = grants.into_iter().find(|g| {
		g.data["targets"]
			.as_array()
			.is_some_and(|a| a.contains(&json!(origin)))
			&& g.data["subjects"] == json!(access.subjects)
			&& eligible(
				access,
				g.data["approver"].as_str().unwrap_or_default(),
				&g.owner,
				&target,
			)
	});
	let id = Uuid::new_v4();
	let designated = grant
		.as_ref()
		.and_then(|g| g.data["approver"].as_str().map(str::to_owned))
		.or(designated);
	let state = if direct || grant.is_some() {
		"approved"
	} else if designated.is_some() {
		"pending"
	} else {
		"blocked"
	};
	let data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":access.identity.subject,"credential_id":access.identity.credential_id,"subjects":access.subjects,"url":input.url,"targets":[origin],"action":"network.get","digest":digest,"policy_revision":access.snapshot.revision,"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	let record = records::insert(
		access,
		id,
		Some(area.id),
		"outbound",
		state,
		data,
		Some(Utc::now() + Duration::seconds(store.capabilities.0.approval_seconds as i64)),
	)
	.await?;
	sessions::cache(
		access,
		input.idempotency_key,
		&digest,
		&json!({"operation_id":id}),
	)
	.await?;
	store.event(&mut access.tx,Some(run.workspace_id),"capability.approval_requested",json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"],"approver":record.data["approver"]})).await?;
	view(store, access, run, &record).await
}
pub(crate) async fn view(
	store: &Store,
	access: &mut Access,
	run: &Run,
	record: &Record,
) -> Result<Value> {
	if record.owner != access.identity.subject || record.data["run_id"] != json!(run.id) {
		return Err(Error::NotFound("operation unavailable".into()));
	}
	let expired = record.expires_at.is_some_and(|t| t < Utc::now());
	let status = match record.state.as_str() {
		"pending" if !expired => "approval_required",
		"pending" | "denied" | "revoked" | "blocked" => "blocked",
		"approved" | "attempted" => "running",
		s => s,
	};
	let mut result = json!({"operation_id":record.id,"approval_id":record.id,"status":status,"approval_state":if expired&&record.state=="pending"{"expired"}else{&record.state},"approval_revision":record.revision,"targets":record.data["targets"],"approver":record.data["approver"],"expires_at":record.expires_at,"grant_id":record.data["grant_id"],"effects_may_have_occurred":record.data["attempted_at"].is_string(),"error":record.data["error"]});
	if let Some(file) = record.data.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		let bytes = store.capabilities.read(access, &entry).await?;
		let text = String::from_utf8_lossy(&bytes);
		let mut end = text.len().min(16384);
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		result["output"] = json!(&text[..end]);
		result["truncated"] = json!(end < text.len());
		result["output_file"] = file.clone();
		result["http_status"] = record.data["http_status"].clone();
	}
	Ok(result)
}
pub(crate) async fn decide(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: ApprovalDecision,
) -> Result<Value> {
	let mut record = records::get(access, id, "outbound").await?;
	let digest = crate::registry::digest(&json!(["approval", id, input]));
	if let Some(result) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	let run_id: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	let origin = record.data["targets"][0].as_str().ok_or(Error::Forbidden)?;
	let target = resource(access, origin, run_id);
	if record.data["approver"] != access.identity.subject
		|| !eligible(access, &access.identity.subject, &record.owner, &target)
	{
		return Err(Error::NotFound("approval unavailable".into()));
	}
	if record.revision != input.expected_revision
		|| record.state != "pending"
		|| record.expires_at.is_none_or(|t| t <= Utc::now())
	{
		return Err(Error::Conflict("APPROVAL_STALE_OR_EXPIRED".into()));
	}
	let run: Run = sqlx::query_as(
		&sessions::select("runs")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run_id)
	.fetch_one(&mut **access.tx)
	.await?;
	if run.control == "CANCELLED"
		|| matches!(run.phase.as_str(), "COMPLETED" | "CANCELLED" | "FAILED")
	{
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let subjects = access.subjects.clone();
	access.subjects = serde_json::from_value(record.data["subjects"].clone())?;
	let allowed = ordinary(access, &target);
	access.subjects = subjects;
	allowed?;
	permitted_origin(store, record.data["url"].as_str().ok_or(Error::Forbidden)?)?;
	match input.choice {
		ApprovalChoice::Deny => record.state = "denied".into(),
		ApprovalChoice::AllowOnce => {
			if input.targets.is_some() || input.expires_at.is_some() {
				return Err(Error::Invalid("allow_once has no reusable scope".into()));
			}
			record.state = "approved".into();
			record.data["approval_mode"] = json!("allow_once");
		}
		ApprovalChoice::AllowRun => {
			let targets = input
				.targets
				.ok_or_else(|| Error::Invalid("explicit targets required".into()))?;
			let expires = input
				.expires_at
				.ok_or_else(|| Error::Invalid("explicit expiry required".into()))?;
			if targets != vec![origin.to_owned()]
				|| expires <= Utc::now()
				|| expires
					> Utc::now() + Duration::seconds(store.capabilities.0.grant_seconds as i64)
			{
				return Err(Error::Invalid("INVALID_GRANT_SCOPE".into()));
			}
			let grant=records::insert(access,Uuid::new_v4(),record.area_id,"grant","active",json!({"run_id":run_id,"agent_id":run.agent_id,"agent_version":run.agent_version,"subjects":record.data["subjects"],"targets":targets,"actions":["network.get"],"approver":access.identity.subject,"requester":record.owner,"approval_id":id}),Some(expires)).await?;
			// Grant ownership belongs to the original requester, including when
			// a designated approver made the decision.
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(Alias::new("core_records"))
					.value(Alias::new("owner"), Expr::cust("$2"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(grant.id)
			.bind(&record.owner)
			.execute(&mut **access.tx)
			.await?;
			record.data["grant_id"] = json!(grant.id);
			record.data["approval_mode"] = json!("allow_run");
			record.state = "approved".into();
		}
	}
	record.data["decided_by"] = json!(access.identity.subject);
	record.data["decision_policy_revision"] = json!(access.snapshot.revision);
	records::update(access, &mut record).await?;
	let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":record.data["grant_id"]});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	store
		.event(
			&mut access.tx,
			Some(run.workspace_id),
			"capability.approval_decided",
			result.clone(),
		)
		.await?;
	Ok(result)
}
pub(crate) async fn revoke(
	access: &mut Access,
	id: Uuid,
	kind: &str,
	input: Revoke,
) -> Result<Value> {
	let mut record = records::get(access, id, kind).await?;
	let digest = crate::registry::digest(&json!(["revoke", id, input]));
	if let Some(result) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	if record.owner != access.identity.subject && record.data["approver"] != access.identity.subject
	{
		return Err(Error::NotFound("grant unavailable".into()));
	}
	if record.revision != input.expected_revision {
		return Err(Error::Conflict("GRANT_CHANGED".into()));
	}
	record.state = "revoked".into();
	records::update(access, &mut record).await?;
	let result = json!({"id":id,"state":"revoked","revision":record.revision,"effects_may_have_occurred":true});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
pub(crate) async fn authorize(store: &Store, access: &mut Access, record: &Record) -> Result<Run> {
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let id = serde_json::from_value(record.data["run_id"].clone())?;
	access.subjects = serde_json::from_value(record.data["subjects"].clone())?;
	let run = access.run_for_interaction(id).await?;
	if record.state != "attempted"
		|| record.expires_at.is_some_and(|t| t <= Utc::now())
		|| record.owner != access.identity.subject
		|| run.control == "CANCELLED"
		|| matches!(run.phase.as_str(), "COMPLETED" | "CANCELLED" | "FAILED")
	{
		return Err(Error::Forbidden);
	}
	sessions::context_authority(access, &run).await?;
	let (_, origin) =
		permitted_origin(store, record.data["url"].as_str().ok_or(Error::Forbidden)?)?;
	let target = resource(access, &origin, id);
	access.require(&target, "capability.request").await?;
	let direct = ordinary(access, &target)?;
	if !direct {
		if !eligible(
			access,
			record.data["approver"].as_str().ok_or(Error::Forbidden)?,
			&record.owner,
			&target,
		) {
			return Err(Error::Forbidden);
		}
		if let Some(grant) = record.data["grant_id"].as_str() {
			let grant = records::get(
				access,
				grant.parse().map_err(|_| Error::Forbidden)?,
				"grant",
			)
			.await?;
			if grant.state != "active"
				|| grant.expires_at.is_none_or(|t| t <= Utc::now())
				|| grant.data["run_id"] != json!(id)
				|| grant.data["subjects"] != json!(access.subjects)
				|| !grant.data["targets"]
					.as_array()
					.is_some_and(|a| a.contains(&json!(origin)))
			{
				return Err(Error::Forbidden);
			}
		} else if record.data["approval_mode"] != "allow_once" {
			return Err(Error::Forbidden);
		}
	}
	Ok(run)
}

//! Approval requests, human decisions and worker authorization retain one live scope.
use crate::{
	Error, Result,
	ports::capabilities::approvals::{ApprovalScope, Creation},
};
use aidash_domain::{
	RunControl, RunMetadata as Run,
	capabilities::{
		approvals::{self, ApprovalChoice, ApprovalDecision, Outbound, Revoke},
		operations::MountedFile as FileEntry,
		outbound::permitted_origin,
		records::Record,
	},
	policy::Resource,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
fn resource(scope: &dyn ApprovalScope, origin: &str, run: Uuid) -> Resource {
	scope.resource(
		"outbound",
		origin,
		json!({"origin":origin,"run_id":run,"action":"network.get"}),
	)
}
fn ordinary(scope: &dyn ApprovalScope, target: &Resource) -> Result<bool> {
	let mut all = true;
	for subject in scope.subjects() {
		let decision = scope
			.bundle()
			.evaluate(&scope.evaluation(subject, target, "network.get"));
		if !decision.allowed && decision.reason != "no_matching_allow" {
			return Err(Error::Forbidden);
		}
		all &= decision.allowed;
	}
	Ok(all)
}
fn eligible(scope: &dyn ApprovalScope, subject: &str, requester: &str, target: &Resource) -> bool {
	let mut evaluation = scope.evaluation(subject, target, "capability.approve");
	evaluation.environment["transport"] = json!("api");
	approvals::approver_eligible(scope.bundle(), &evaluation, requester)
}
fn approver(scope: &dyn ApprovalScope, target: &Resource) -> Option<String> {
	if eligible(scope, scope.principal(), scope.principal(), target) {
		return Some(scope.principal().into());
	}
	scope
		.bundle()
		.subjects
		.keys()
		.find(|id| eligible(scope, id, scope.principal(), target))
		.cloned()
}
pub async fn visible(scope: &mut dyn ApprovalScope, record: &Record) -> Result<bool> {
	if record.owner == scope.principal() {
		return Ok(true);
	}
	if record.data["approver"] != scope.principal() {
		return Ok(false);
	}
	let run: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	scope.run_context(run).await?;
	let Some(targets) = record.data["targets"]
		.as_array()
		.filter(|items| !items.is_empty())
	else {
		return Ok(false);
	};
	for origin in targets {
		let Some(origin) = origin.as_str() else {
			return Ok(false);
		};
		if !eligible(
			scope,
			scope.principal(),
			&record.owner,
			&resource(scope, origin, run),
		) {
			return Ok(false);
		}
	}
	Ok(true)
}
pub async fn prepare(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	area: Uuid,
	input: Outbound,
) -> Result<Value> {
	let limits = scope.limits()?;
	let (_, origin) = permitted_origin(&input.url, &limits.origins)?;
	let digest = aidash_domain::registry::rules::digest(&json!(["outbound_get", run.id, input]));
	if let Some(previous) = scope.cached(input.idempotency_key, &digest).await? {
		let id = serde_json::from_value(previous["operation_id"].clone())?;
		let previous = scope.load(id, "outbound").await?;
		return view(scope, run, &previous).await;
	}
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if scope.active_run().await? != Some(run.id) || run.phase.is_terminal() {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let target = resource(scope, &origin, run.id);
	scope.require(&target, "capability.request").await?;
	let direct = ordinary(scope, &target)?;
	let designated = approver(scope, &target);
	let grants = scope.grants(run.id).await?;
	let grant = grants.into_iter().find(|g| {
		g.data["targets"]
			.as_array()
			.is_some_and(|a| a.contains(&json!(origin)))
			&& g.data["subjects"] == json!(scope.subjects())
			&& eligible(
				scope,
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
	let state = approvals::requested_state(direct, grant.is_some(), designated.is_some());
	let data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),"url":input.url,"targets":[origin],"action":"network.get","digest":digest,"policy_revision":scope.policy_revision(),"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	let record = scope
		.insert(Creation {
			id,
			area: Some(area),
			kind: "outbound",
			state,
			data,
			expires: Some(Utc::now() + Duration::seconds(limits.approval_seconds as i64)),
		})
		.await?;
	scope
		.cache(input.idempotency_key, &digest, &json!({"operation_id":id}))
		.await?;
	scope.event(run.workspace_id,"capability.approval_requested",json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"],"approver":record.data["approver"]})).await?;
	view(scope, run, &record).await
}
pub async fn view(scope: &mut dyn ApprovalScope, run: &Run, record: &Record) -> Result<Value> {
	if record.owner != scope.principal() || record.data["run_id"] != json!(run.id) {
		return Err(Error::NotFound("operation unavailable".into()));
	}
	let expired = record.expires_at.is_some_and(|t| t < Utc::now());
	let mut result = approvals::projection(record, expired);
	if let Some(file) = record.data.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		let bytes = scope.read(&entry).await?;
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
pub async fn decide(
	scope: &mut dyn ApprovalScope,
	id: Uuid,
	input: ApprovalDecision,
) -> Result<Value> {
	let limits = scope.limits()?;
	let mut record = scope.load(id, "outbound").await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["approval", id, input]));
	if let Some(result) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	let run_id: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	let run = scope.run_context(run_id).await?;
	let origin = record.data["targets"][0].as_str().ok_or(Error::Forbidden)?;
	let target = resource(scope, origin, run_id);
	if record.data["approver"] != scope.principal()
		|| !eligible(scope, scope.principal(), &record.owner, &target)
	{
		return Err(Error::NotFound("approval unavailable".into()));
	}
	if record.revision != input.expected_revision
		|| record.state != "pending"
		|| record.expires_at.is_none_or(|t| t <= Utc::now())
	{
		return Err(Error::Conflict("APPROVAL_STALE_OR_EXPIRED".into()));
	}
	if run.control == RunControl::Cancelled || run.phase.is_terminal() {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let subjects = scope.replace_subjects(serde_json::from_value(record.data["subjects"].clone())?);
	let allowed = ordinary(scope, &target);
	scope.replace_subjects(subjects);
	allowed?;
	permitted_origin(
		record.data["url"].as_str().ok_or(Error::Forbidden)?,
		&limits.origins,
	)?;
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
				|| expires > Utc::now() + Duration::seconds(limits.grant_seconds as i64)
			{
				return Err(Error::Invalid("INVALID_GRANT_SCOPE".into()));
			}
			let grant=scope.insert(Creation {id:Uuid::new_v4(),area:record.area_id,kind:"grant",state:"active",data:json!({"run_id":run_id,"agent_id":run.agent_id,"agent_version":run.agent_version,"subjects":record.data["subjects"],"targets":targets,"actions":["network.get"],"approver":scope.principal(),"requester":record.owner,"approval_id":id}),expires:Some(expires)}).await?;
			scope.transfer_owner(grant.id, &record.owner).await?;
			record.data["grant_id"] = json!(grant.id);
			record.data["approval_mode"] = json!("allow_run");
			record.state = "approved".into();
		}
	}
	record.data["decided_by"] = json!(scope.principal());
	record.data["decision_policy_revision"] = json!(scope.policy_revision());
	scope.update(&mut record).await?;
	let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":record.data["grant_id"]});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope
		.event(
			run.workspace_id,
			"capability.approval_decided",
			result.clone(),
		)
		.await?;
	Ok(result)
}
pub async fn revoke(
	scope: &mut dyn ApprovalScope,
	id: Uuid,
	kind: &str,
	input: Revoke,
) -> Result<Value> {
	let mut record = scope.load(id, kind).await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["revoke", id, input]));
	if let Some(result) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	if record.owner != scope.principal() && record.data["approver"] != scope.principal() {
		return Err(Error::NotFound("grant unavailable".into()));
	}
	if record.revision != input.expected_revision {
		return Err(Error::Conflict("GRANT_CHANGED".into()));
	}
	record.state = "revoked".into();
	scope.update(&mut record).await?;
	let result = json!({"id":id,"state":"revoked","revision":record.revision,"effects_may_have_occurred":true});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
pub async fn authorize(scope: &mut dyn ApprovalScope, record: &Record) -> Result<Run> {
	let limits = scope.limits()?;
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	let id = serde_json::from_value(record.data["run_id"].clone())?;
	scope.replace_subjects(serde_json::from_value(record.data["subjects"].clone())?);
	let run = scope.interaction_run(id).await?;
	if record.state != "attempted"
		|| record.expires_at.is_some_and(|t| t <= Utc::now())
		|| record.owner != scope.principal()
		|| run.control == RunControl::Cancelled
		|| run.phase.is_terminal()
	{
		return Err(Error::Forbidden);
	}
	scope.context_authority().await?;
	let (_, origin) = permitted_origin(
		record.data["url"].as_str().ok_or(Error::Forbidden)?,
		&limits.origins,
	)?;
	let target = resource(scope, &origin, id);
	scope.require(&target, "capability.request").await?;
	let direct = ordinary(scope, &target)?;
	if !direct {
		if !eligible(
			scope,
			record.data["approver"].as_str().ok_or(Error::Forbidden)?,
			&record.owner,
			&target,
		) {
			return Err(Error::Forbidden);
		}
		if let Some(grant) = record.data["grant_id"].as_str() {
			let grant = scope
				.load(grant.parse().map_err(|_| Error::Forbidden)?, "grant")
				.await?;
			if grant.state != "active"
				|| grant.expires_at.is_none_or(|t| t <= Utc::now())
				|| grant.data["run_id"] != json!(id)
				|| grant.data["subjects"] != json!(scope.subjects())
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

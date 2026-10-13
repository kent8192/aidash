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
/// Approval records of Web disclosures share the capability approval flow.
pub const DISCLOSURE: &str = "disclosure";
/// The policy action a Web disclosure approval authorizes.
pub const DISCLOSE: &str = "research.disclose";

fn disclosure_resource(scope: &dyn ApprovalScope, destination: &str, run: Uuid) -> Resource {
	scope.resource(
		"research",
		destination,
		json!({"destination":destination,"run_id":run,"action":DISCLOSE}),
	)
}
/// The protected target of an approval record and the action it authorizes.
fn target_of(
	scope: &dyn ApprovalScope,
	record: &Record,
	target: &str,
	run: Uuid,
) -> (Resource, &'static str) {
	// A Run grant from an "allow for this Run" disclosure decision names the
	// action it covers; it protects the same research resource.
	let granted_disclosure = record.kind == "grant"
		&& record.data["actions"]
			.as_array()
			.is_some_and(|actions| actions.contains(&json!(DISCLOSE)));
	if record.kind == DISCLOSURE || granted_disclosure {
		(disclosure_resource(scope, target, run), DISCLOSE)
	} else {
		(resource(scope, target, run), "network.get")
	}
}
fn ordinary(scope: &dyn ApprovalScope, target: &Resource, action: &str) -> Result<bool> {
	let mut all = true;
	for subject in scope.subjects() {
		let decision = scope
			.bundle()
			.evaluate(&scope.evaluation(subject, target, action));
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
			&target_of(scope, record, origin, run).0,
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
	let direct = ordinary(scope, &target, "network.get")?;
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
	let mut record = match scope.load(id, "outbound").await {
		Err(Error::NotFound(_)) => scope.load(id, DISCLOSURE).await?,
		other => other?,
	};
	let disclosure = record.kind == DISCLOSURE;
	let digest = aidash_domain::registry::rules::digest(&json!(["approval", id, input]));
	if let Some(result) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	let run_id: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	let run = scope.run_context(run_id).await?;
	let origin = record.data["targets"][0].as_str().ok_or(Error::Forbidden)?;
	let (target, action) = target_of(scope, &record, origin, run_id);
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
	let allowed = ordinary(scope, &target, action);
	scope.replace_subjects(subjects);
	allowed?;
	if !disclosure {
		permitted_origin(
			record.data["url"].as_str().ok_or(Error::Forbidden)?,
			&limits.origins,
		)?;
	}
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
			let grant=scope.insert(Creation {id:Uuid::new_v4(),area:record.area_id,kind:"grant",state:"active",data:json!({"run_id":run_id,"agent_id":run.agent_id,"agent_version":run.agent_version,"subjects":record.data["subjects"],"targets":targets,"actions":[action],"approver":scope.principal(),"requester":record.owner,"approval_id":id,"context_revision":record.data["context_revision"]}),expires:Some(expires)}).await?;
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
	let direct = ordinary(scope, &target, "network.get")?;
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

/// One exact Web disclosure awaiting or holding approval.
#[derive(Debug, Clone)]
pub struct DisclosureRequest {
	pub digest: String,
	pub operation: aidash_domain::research::Operation,
	pub provider: String,
	pub destination: String,
	pub content: String,
	/// The Run's observed input sequence. Approvals and Run grants cover only
	/// disclosures made before new user input.
	pub context_revision: i64,
}

fn disclosure_key(digest: &str, generation: u32) -> Uuid {
	let bytes = aidash_domain::registry::rules::digest(&json!([DISCLOSURE, digest, generation]));
	let hex = bytes.trim_start_matches("sha256:");
	let mut id = [0u8; 16];
	for (index, byte) in id.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or_default();
	}
	uuid::Builder::from_random_bytes(id).into_uuid()
}

/// The newest recorded generation of one exact disclosure and its cache entry.
/// Generations are recorded in order, and a new one is recorded only once every
/// earlier one is consumed, so the newest alone decides admission. Exponential
/// then binary search keeps the lookups logarithmic in the generations used.
async fn newest_disclosure(
	scope: &mut dyn ApprovalScope,
	digest: &str,
) -> Result<Option<(u32, Value)>> {
	let Some(mut newest) = scope.cached(disclosure_key(digest, 0), digest).await? else {
		return Ok(None);
	};
	// `low` is recorded; `high` is not, or is past the last representable one.
	let (mut low, mut high) = (0u32, 1u32);
	while let Some(entry) = scope.cached(disclosure_key(digest, high), digest).await? {
		(low, newest) = (high, entry);
		let Some(next) = high.checked_mul(2) else {
			high = u32::MAX;
			break;
		};
		high = next;
	}
	while high - low > 1 {
		let middle = low + (high - low) / 2;
		match scope.cached(disclosure_key(digest, middle), digest).await? {
			Some(entry) => (low, newest) = (middle, entry),
			None => high = middle,
		}
	}
	Ok(Some((low, newest)))
}

/// Whether an approved disclosure of `run` still admits what it approved: the
/// approval has not expired, current policy does not forbid the disclosure,
/// and unless policy now permits it directly, its approver is still eligible
/// and an "allow for this Run" decision's grant is still active. As
/// [`authorize`] does for outbound reads, a decision never outlives these.
async fn still_approved(scope: &mut dyn ApprovalScope, run: &Run, record: &Record) -> Result<bool> {
	if record.expires_at.is_none_or(|t| t <= Utc::now()) {
		return Ok(false);
	}
	let Some(destination) = record.data["destination"].as_str() else {
		return Ok(false);
	};
	let target = disclosure_resource(scope, destination, run.id);
	match ordinary(scope, &target, DISCLOSE) {
		Ok(true) => return Ok(true),
		Ok(false) => {}
		Err(Error::Forbidden) => return Ok(false),
		Err(error) => return Err(error),
	}
	let Some(approver) = record.data["approver"].as_str() else {
		return Ok(false);
	};
	if !eligible(scope, approver, &record.owner, &target) {
		return Ok(false);
	}
	let Some(grant) = record.data["grant_id"].as_str() else {
		return Ok(record.data["approval_mode"] == "allow_once");
	};
	let Ok(grant) = grant.parse() else {
		return Ok(false);
	};
	let grant = match scope.load(grant, "grant").await {
		Err(Error::NotFound(_)) => return Ok(false),
		other => other?,
	};
	Ok(grant.state == "active"
		&& grant.expires_at.is_some_and(|t| t > Utc::now())
		&& grant.data["run_id"] == json!(run.id)
		&& grant.data["targets"]
			.as_array()
			.is_some_and(|targets| targets.contains(&json!(destination))))
}

/// Admit one exact Web disclosure of a Run. Policy may permit it directly or
/// through a Run grant; otherwise a designated approver decides through the
/// ordinary approval decision, and the Run waits on the returned approval.
/// An approval is consumed by the one disclosure it allows; repeating the same
/// disclosure later needs a new approval.
pub async fn disclose(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	request: &DisclosureRequest,
) -> Result<crate::ports::research::DisclosureState> {
	use crate::ports::research::DisclosureState;
	let limits = scope.limits()?;
	if run.phase.is_terminal() || run.control == RunControl::Cancelled {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let target = disclosure_resource(scope, &request.destination, run.id);
	match scope.require(&target, "capability.request").await {
		Err(Error::Forbidden) => return Ok(DisclosureState::Denied),
		other => other?,
	}
	let generation = match newest_disclosure(scope, &request.digest).await? {
		None => 0,
		Some((generation, previous)) => {
			let id = serde_json::from_value(previous["operation_id"].clone())?;
			let record = scope.load(id, DISCLOSURE).await?;
			let expired = record.expires_at.is_some_and(|t| t <= Utc::now());
			match record.state.as_str() {
				// Every otherwise-admissible repetition gets its own generation.
				"consumed" => generation
					.checked_add(1)
					.ok_or_else(|| Error::Conflict("DISCLOSURE_GENERATIONS_EXHAUSTED".into()))?,
				// Consumed only once the attempt is reserved; see consume_disclosure.
				// An expired or no longer authorized approval admits nothing.
				"approved" => {
					return Ok(if still_approved(scope, run, &record).await? {
						DisclosureState::Approved {
							approval_id: record.id,
						}
					} else {
						DisclosureState::Denied
					});
				}
				"pending" if !expired => {
					return Ok(DisclosureState::Pending {
						approval_id: record.id,
						expires_at: record.expires_at,
					});
				}
				_ => return Ok(DisclosureState::Denied),
			}
		}
	};
	let direct = match ordinary(scope, &target, DISCLOSE) {
		Err(Error::Forbidden) => return Ok(DisclosureState::Denied),
		other => other?,
	};
	let grant = scope.grants(run.id).await?.into_iter().find(|g| {
		g.data["targets"]
			.as_array()
			.is_some_and(|a| a.contains(&json!(request.destination)))
			&& g.data["actions"]
				.as_array()
				.is_some_and(|a| a.contains(&json!(DISCLOSE)))
			&& g.data["subjects"] == json!(scope.subjects())
			// New user input needs a new decision, as for one-time approvals.
			&& g.data["context_revision"] == json!(request.context_revision)
			&& eligible(
				scope,
				g.data["approver"].as_str().unwrap_or_default(),
				&g.owner,
				&target,
			)
	});
	let designated = grant
		.as_ref()
		.and_then(|g| g.data["approver"].as_str().map(str::to_owned))
		.or_else(|| approver(scope, &target));
	let state = approvals::requested_state(direct, grant.is_some(), designated.is_some());
	let id = Uuid::new_v4();
	let data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),
		"operation":request.operation,"provider":request.provider,"destination":request.destination,"content":request.content,"context_revision":request.context_revision,"targets":[request.destination],"action":DISCLOSE,
		"digest":request.digest,"policy_revision":scope.policy_revision(),"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	let record = scope
		.insert(Creation {
			id,
			area: None,
			kind: DISCLOSURE,
			// A direct or granted disclosure is consumed by this call.
			state: if state == "approved" {
				"consumed"
			} else {
				state
			},
			data,
			expires: Some(Utc::now() + Duration::seconds(limits.approval_seconds as i64)),
		})
		.await?;
	scope
		.cache(
			disclosure_key(&request.digest, generation),
			&request.digest,
			&json!({"operation_id":id}),
		)
		.await?;
	scope.event(run.workspace_id,"capability.approval_requested",json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"],"approver":record.data["approver"],"action":DISCLOSE})).await?;
	Ok(match state {
		"approved" => DisclosureState::Permitted,
		"pending" => DisclosureState::Pending {
			approval_id: id,
			expires_at: record.expires_at,
		},
		_ => DisclosureState::Denied,
	})
}

/// Consume a one-time Web disclosure approval of `run`, owned by the current
/// principal. `false` when it is no longer an unconsumed approval that is
/// unexpired and still authorized; the disclosure must then not be sent.
pub async fn consume_disclosure(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	id: Uuid,
) -> Result<bool> {
	let mut record = match scope.load(id, DISCLOSURE).await {
		Err(Error::NotFound(_)) => return Ok(false),
		other => other?,
	};
	if record.owner != scope.principal()
		|| record.data["run_id"] != json!(run.id)
		|| record.state != "approved"
		|| !still_approved(scope, run, &record).await?
	{
		return Ok(false);
	}
	record.state = "consumed".into();
	record.data["consumed_at"] = json!(Utc::now());
	scope.update(&mut record).await?;
	Ok(true)
}

#[cfg(test)]
mod tests;

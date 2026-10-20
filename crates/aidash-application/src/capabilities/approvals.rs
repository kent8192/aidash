//! Approval requests, human decisions and worker authorization retain one live scope.
pub use crate::ports::capabilities::approvals::{
	JudgeRequest, Refusal, Settlement, ToolCallApproval, ToolCallRequest,
};
use crate::{
	Error, Result,
	ports::capabilities::approvals::{ApprovalScope, Creation},
};
use aidash_domain::{
	RunControl, RunMetadata as Run,
	approval_rules::{
		APPROVAL_ADVANCE, ApprovalKind, ApprovalMode, ApprovalRule, Ceiling, GivenBy,
		MODE_RESOURCE, RefusedBy, Resolution, RuleEffect, RuleTarget, Settings, Standing,
		TenantStance, resolve,
	},
	capabilities::{
		approvals::{self, ApprovalChoice, ApprovalDecision, Outbound, Revoke},
		operations::MountedFile as FileEntry,
		outbound::permitted_origin,
		records::Record,
	},
	policy::{PolicyBundle, Resource, SubjectKind},
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
/// Approval records of outbound reads.
const OUTBOUND: &str = "outbound";
/// The kind of every approval record a Run can hold.
pub const KINDS: [&str; 3] = [OUTBOUND, DISCLOSURE, TOOL_CALL];
/// Approval records of Web disclosures share the capability approval flow.
pub const DISCLOSURE: &str = "disclosure";
/// Approval records of Tenant tool calls share it too (ADR 0075).
pub const TOOL_CALL: &str = "tool_call";
/// What the model is told when it asks a question of a person who asked not
/// to be asked (Approval Mode Don't ask).
pub const DONT_ASK_HUMAN_REQUEST: &str =
	"the person asked not to be asked; continue on your own judgment";
/// What the system answers a model's question with once the person's Answer
/// Deadline passed before anyone answered it.
pub fn answer_deadline_response() -> Value {
	json!({"answered": false, "reason": "answer_deadline"})
}
/// What the model reads when its question went unanswered until the person's
/// Answer Deadline.
pub const ANSWER_DEADLINE_PASSED: &str =
	"no answer arrived within the person's Answer Deadline; continue on your own judgment";
/// The answer to a model's question as the model reads it: one the system gave
/// at the Answer Deadline also tells the model to go on.
pub fn answer_for_model(response: Value) -> Value {
	if response != answer_deadline_response() {
		return response;
	}
	let mut told = response;
	told["message"] = json!(ANSWER_DEADLINE_PASSED);
	told
}
/// What the model is told while the person's Approval Mode is Auto, so a Run
/// the Decider approves is not held up by questions.
pub const AUTO_MODE_INSTRUCTION: &str = "\nThe person has chosen the Auto Approval Mode. Do not stop to ask clarifying questions unless the person's input relies on one; make a reasonable assumption and continue.";
/// The policy action that lets a call proceed without a Tool call approval
/// (direct permission).
pub const CALL_UNAPPROVED: &str = "tool.call_unapproved";
/// The operation of a Tenant's HTTP or MCP tool definition, until a definition
/// version can declare several (#410).
pub const INVOKE: &str = "invoke";
/// The policy action a Web disclosure approval authorizes.
pub const DISCLOSE: &str = "research.disclose";
/// The shortest Run grant an approver may give, and so the least a
/// Deployment's `grant_seconds` may be. The dashboard offers whole minutes;
/// a few seconds of slack let a one-minute grant survive the trip to the
/// server.
pub const MIN_RUN_GRANT_SECONDS: i64 = 60;
const GRANT_CLOCK_SLACK_SECONDS: i64 = 5;

fn disclosure_resource(scope: &dyn ApprovalScope, destination: &str, run: Uuid) -> Resource {
	scope.resource(
		"research",
		destination,
		json!({"destination":destination,"run_id":run,"action":DISCLOSE}),
	)
}
/// The target string of an approval record and Run grant for one operation of
/// a tool. It cannot collide with an outbound origin or a disclosure destination.
pub fn tool_target(tool: &aidash_domain::registry::EntityRef, operation: &str) -> String {
	format!("tool:{}@{}#{operation}", tool.id, tool.version)
}
/// The target of an Approval Rule for one operation of a tool. A rule names
/// the tool, never one of its versions.
pub fn tool_rule_target(tool: &str, operation: &str) -> String {
	format!("tool:{tool}#{operation}")
}
/// The `approval_mode` of a record whose approval a person's Approval Rule or
/// Approval Mode gave in advance. Like "allow_once", it covers one use.
const ADVANCE: &str = "advance";
/// Whether `record` is an approval of one use: a person's "allow once", or what
/// their rule or mode gave in advance.
fn once(record: &Record) -> bool {
	matches!(
		record.data["approval_mode"].as_str(),
		Some("allow_once" | ADVANCE)
	)
}

/// What the person the Run's delegation chain starts at decided in advance
/// about an approval the Tenant's policy leaves to a person.
enum Advice {
	/// Their rule or mode gave the approval.
	Approve { by: GivenBy, person: String },
	/// Their rule or mode refused it.
	Refuse { by: RefusedBy, person: String },
	/// Their Auto mode leaves it to the Tenant's approval Decider, which settles
	/// the record afterwards.
	Judge,
	/// Nothing decides it in advance.
	Ask,
}

impl Advice {
	/// The state of a new record: `approved` for what is given, `denied` for
	/// what is refused, `pending` for what the Decider is to judge, `None` when a
	/// person has yet to decide.
	fn state(&self, approved: &'static str) -> Option<&'static str> {
		match self {
			Self::Approve { .. } => Some(approved),
			Self::Refuse { .. } => Some("denied"),
			Self::Judge => Some("pending"),
			Self::Ask => None,
		}
	}
	/// Record under whose name, and by which rule or mode, the decision was made.
	fn record(&self, data: &mut Value) {
		let person = match self {
			Self::Approve { by, person } => {
				data["approver"] = json!(person);
				data["approval_mode"] = json!(ADVANCE);
				data["given_by"] = by.data();
				person
			}
			Self::Refuse { by, person } => {
				data["refused_by"] = by.data();
				data["error"] = json!(by.code());
				person
			}
			Self::Judge => {
				data["judge"] = json!(true);
				return;
			}
			Self::Ask => return,
		};
		data["decided_by"] = json!(person);
		data["decided_at"] = json!(Utc::now());
	}
}

/// The Approval Rule kind and target a Web disclosure is judged by.
fn disclosure_rule(
	operation: aidash_domain::research::Operation,
	provider: &str,
	destination: &str,
) -> (ApprovalKind, String) {
	match operation {
		aidash_domain::research::Operation::Search => (ApprovalKind::WebSearch, provider.into()),
		_ => (ApprovalKind::PageReading, destination.into()),
	}
}

/// What the Tenant's policy lets `root`, judged alone, approve in advance.
fn ceiling(
	scope: &dyn ApprovalScope,
	settings: &Settings,
	root: &str,
	target: &Resource,
) -> Ceiling {
	let allows = |resource: &Resource, action: &str| {
		scope
			.bundle()
			.evaluate(&scope.evaluation(root, resource, action))
			.allowed
	};
	settings.ceiling(Standing {
		approver: eligible(scope, root, scope.principal(), target),
		advance: allows(target, APPROVAL_ADVANCE),
		mode: settings
			.mode
			.policy_requirement()
			.is_none_or(|(action, id)| {
				allows(&scope.resource(MODE_RESOURCE, id, json!({})), action)
			}),
	})
}

/// Decide, by the Run's person's Approval Rules and Approval Mode, an approval
/// of `kind` for `name` that neither the Tenant's policy nor a Run grant
/// already covers. `target` is the resource the approval protects.
async fn advise(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	kind: ApprovalKind,
	name: &str,
	target: &Resource,
) -> Result<Advice> {
	let settings = scope.approval_settings(run).await?;
	let root = scope.chain_root().map(str::to_owned);
	let (Some(settings), Some(person)) = (settings, root) else {
		return Ok(Advice::Ask);
	};
	let ceiling = ceiling(scope, &settings, &person, target);
	let auto_suspended =
		settings.mode == ApprovalMode::Auto && scope.auto_blocks(run).await?.suspended();
	Ok(
		match resolve(
			TenantStance::NeedsApproval,
			Some(&settings),
			ceiling,
			kind,
			name,
			auto_suspended,
		) {
			Resolution::Approve(by) => Advice::Approve { by, person },
			Resolution::Refuse(by) => Advice::Refuse { by, person },
			Resolution::Judge => Advice::Judge,
			Resolution::Ask | Resolution::Proceed => Advice::Ask,
		},
	)
}
fn tool_resource(
	scope: &dyn ApprovalScope,
	tool: &aidash_domain::registry::EntityRef,
	operation: &str,
	run: Uuid,
) -> Resource {
	scope.resource(
		"tool",
		&tool.id,
		json!({"version":tool.version,"operation":operation,"run_id":run,"action":CALL_UNAPPROVED}),
	)
}
/// Whether `record` is a Run grant for `action`.
fn grants(record: &Record, action: &str) -> bool {
	record.kind == "grant"
		&& record.data["actions"]
			.as_array()
			.is_some_and(|actions| actions.contains(&json!(action)))
}
/// The protected target of an approval record and the action it authorizes.
fn target_of(
	scope: &dyn ApprovalScope,
	record: &Record,
	target: &str,
	run: Uuid,
) -> (Resource, &'static str) {
	// A Run grant from an "allow for this Run" decision names the action it
	// covers; it protects the same resource as the approval it came from.
	if record.kind == TOOL_CALL || grants(record, CALL_UNAPPROVED) {
		let tool = aidash_domain::registry::EntityRef {
			id: record.data["tool"].as_str().unwrap_or_default().into(),
			version: record.data["version"].as_str().unwrap_or_default().into(),
		};
		let operation = record.data["operation"].as_str().unwrap_or_default();
		(tool_resource(scope, &tool, operation, run), CALL_UNAPPROVED)
	} else if record.kind == DISCLOSURE || grants(record, DISCLOSE) {
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
			// Outbound reads, Web disclosures and tool calls never share a grant.
			&& g.data["actions"]
				.as_array()
				.is_some_and(|a| a.contains(&json!("network.get")))
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
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		advise(scope, run, ApprovalKind::Outbound, &origin, &target).await?
	};
	let state = approvals::requested_state(direct, grant.is_some(), designated.is_some());
	let state = advice.state("approved").unwrap_or(state);
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),"url":input.url,"targets":[origin],"action":"network.get","digest":digest,"policy_revision":scope.policy_revision(),"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	advice.record(&mut data);
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
	if record.kind == TOOL_CALL {
		// What the approver is asked to allow: the exact call.
		for field in ["tool", "version", "operation", "call"] {
			result[field] = record.data[field].clone();
		}
	}
	// Which rule or mode gave or refused the approval, when one did.
	for field in ["given_by", "refused_by"] {
		if let Some(decision) = record.data.get(field) {
			result[field] = decision.clone();
		}
	}
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
/// The Approval Rule kind and target that "always allow" creates for `record`.
fn rule_of(record: &Record) -> Result<(ApprovalKind, String)> {
	let data = &record.data;
	let text = |value: &Value| {
		value
			.as_str()
			.map(str::to_owned)
			.ok_or_else(|| Error::Invalid("approval record is missing its target".into()))
	};
	Ok(match record.kind.as_str() {
		OUTBOUND => (ApprovalKind::Outbound, text(&data["targets"][0])?),
		DISCLOSURE => {
			let operation = serde_json::from_value(data["operation"].clone())?;
			disclosure_rule(
				operation,
				&text(&data["provider"])?,
				&text(&data["destination"])?,
			)
		}
		_ => (
			ApprovalKind::ToolCall,
			tool_rule_target(&text(&data["tool"])?, &text(&data["operation"])?),
		),
	})
}
/// Approval records of every kind share one decision API.
async fn load_approval(scope: &mut dyn ApprovalScope, id: Uuid) -> Result<Record> {
	match scope.load(id, OUTBOUND).await {
		Err(Error::NotFound(_)) => {}
		other => return other,
	}
	match scope.load(id, DISCLOSURE).await {
		Err(Error::NotFound(_)) => {}
		other => return other,
	}
	scope.load(id, TOOL_CALL).await
}
pub async fn decide(
	scope: &mut dyn ApprovalScope,
	id: Uuid,
	input: ApprovalDecision,
) -> Result<Value> {
	let limits = scope.limits()?;
	let mut record = load_approval(scope, id).await?;
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
		// A disclosure or tool call approval ends with its Run. One its Run's
		// end did not already close is closed now, with the reason, instead of
		// failing forever.
		if record.kind != OUTBOUND {
			record.state = "closed".into();
			record.data["closed_reason"] = json!("run_ended");
			scope.update(&mut record).await?;
			let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":Value::Null});
			scope.cache(input.idempotency_key, &digest, &result).await?;
			return Ok(result);
		}
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let subjects = scope.replace_subjects(serde_json::from_value(record.data["subjects"].clone())?);
	let allowed = ordinary(scope, &target, action);
	scope.replace_subjects(subjects);
	allowed?;
	if record.kind == OUTBOUND {
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
		ApprovalChoice::AllowAlways => {
			if input.targets.is_some() || input.expires_at.is_some() {
				return Err(Error::Invalid("allow_always has no reusable scope".into()));
			}
			// Only the person the chain starts at can give an approval in advance.
			let owner = scope.principal().to_owned();
			if record.data["subjects"][0].as_str() != Some(owner.as_str()) {
				return Err(Error::Forbidden);
			}
			// A rule approves in advance only within the ceiling `resolve`
			// applies to it. Without `approval.advance` on this resource the
			// rule would be stored and ignored, so "Always allow" cannot mean
			// what it says: refuse it and leave "Allow once" available.
			let settings = scope
				.approval_settings(&run)
				.await?
				.ok_or(Error::Forbidden)?;
			if !ceiling(scope, &settings, &owner, &target).may_approve {
				return Err(Error::Forbidden);
			}
			let (kind, name) = rule_of(&record)?;
			let rule = ApprovalRule {
				id: Uuid::new_v4(),
				kind,
				target: RuleTarget::Exact(name),
				effect: RuleEffect::Allow,
			};
			// Two pending approvals can name one rule. Record the rule the
			// store actually holds, which an equal one may already be.
			record.data["rule_id"] = json!(scope.create_rule(&owner, rule).await?);
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
			let now = Utc::now();
			if targets != vec![origin.to_owned()]
				|| expires
					< now + Duration::seconds(MIN_RUN_GRANT_SECONDS - GRANT_CLOCK_SLACK_SECONDS)
				|| expires > now + Duration::seconds(limits.grant_seconds as i64)
			{
				return Err(Error::Invalid("INVALID_GRANT_SCOPE".into()));
			}
			let mut data = json!({"run_id":run_id,"agent_id":run.agent_id,"agent_version":run.agent_version,"subjects":record.data["subjects"],"targets":targets,"actions":[action],"approver":scope.principal(),"requester":record.owner,"approval_id":id,"operation":record.data["operation"]});
			// What a grant covers beyond its destination depends on the approval.
			// A tool grant outlives new user input, so it has no context revision.
			if record.kind == TOOL_CALL {
				data["tool"] = record.data["tool"].clone();
				data["version"] = record.data["version"].clone();
			} else {
				data["context_revision"] = record.data["context_revision"].clone();
				data["provider"] = record.data["provider"].clone();
				data["attribution"] = record.data["attribution"].clone();
			}
			let grant = scope
				.insert(Creation {
					id: Uuid::new_v4(),
					area: record.area_id,
					kind: "grant",
					state: "active",
					data,
					expires: Some(expires),
				})
				.await?;
			scope.transfer_owner(grant.id, &record.owner).await?;
			record.data["grant_id"] = json!(grant.id);
			record.data["approval_mode"] = json!("allow_run");
			record.state = "approved".into();
		}
	}
	record.data["decided_by"] = json!(scope.principal());
	record.data["decided_at"] = json!(Utc::now());
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
				|| !grant.data["actions"]
					.as_array()
					.is_some_and(|a| a.contains(&json!("network.get")))
				|| grant.data["subjects"] != json!(scope.subjects())
				|| !grant.data["targets"]
					.as_array()
					.is_some_and(|a| a.contains(&json!(origin)))
			{
				return Err(Error::Forbidden);
			}
		} else if !once(record) {
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
	/// The Run's Working Area, so the approval appears in its Thread.
	pub area: Option<Uuid>,
	/// The Research account holder's account and price when approval is
	/// requested; the approver sees the price, not the current terms.
	pub attribution: Option<aidash_domain::research::account::CostAttribution>,
	/// The validated search query in provider-independent form.
	pub query: Option<Value>,
}

/// A cache key derived from a digest, in the namespace of one approval kind.
fn derived_key(parts: Value) -> Uuid {
	let bytes = aidash_domain::registry::rules::digest(&parts);
	let hex = bytes.trim_start_matches("sha256:");
	let mut id = [0u8; 16];
	for (index, byte) in id.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or_default();
	}
	uuid::Builder::from_random_bytes(id).into_uuid()
}
fn disclosure_key(digest: &str, generation: u32) -> Uuid {
	derived_key(json!([DISCLOSURE, digest, generation]))
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

/// Whether an approved disclosure or tool call of `run` still admits what it
/// approved: the approval has not expired, current policy does not forbid it,
/// and unless policy now permits it directly, its approver is still eligible
/// and an "allow for this Run" decision's grant is still active. As
/// [`authorize`] does for outbound reads, a decision never outlives these.
async fn still_approved(scope: &mut dyn ApprovalScope, run: &Run, record: &Record) -> Result<bool> {
	if record.expires_at.is_none_or(|t| t <= Utc::now()) {
		return Ok(false);
	}
	let Some(destination) = record.data["targets"][0].as_str() else {
		return Ok(false);
	};
	let (target, action) = target_of(scope, record, destination, run.id);
	match ordinary(scope, &target, action) {
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
		return Ok(once(record));
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
			// A grant covers what the approver was told it covers: a search
			// grant only later searches through its provider, charged to the
			// same Research account at the same price, a Page reading grant
			// only page readings of its host, through any provider.
			&& g.data["operation"] == json!(request.operation)
			&& (request.operation != aidash_domain::research::Operation::Search
				|| (g.data["provider"] == json!(request.provider)
					&& g.data["attribution"] == json!(request.attribution)))
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
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		let (kind, name) =
			disclosure_rule(request.operation, &request.provider, &request.destination);
		advise(scope, run, kind, &name, &target).await?
	};
	let state = approvals::requested_state(direct, grant.is_some(), designated.is_some());
	let state = advice.state("approved").unwrap_or(state);
	let id = Uuid::new_v4();
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),
		"operation":request.operation,"provider":request.provider,"destination":request.destination,"content":request.content,"context_revision":request.context_revision,"targets":[request.destination],"action":DISCLOSE,"attribution":request.attribution,"query":request.query,
		"digest":request.digest,"policy_revision":scope.policy_revision(),"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	advice.record(&mut data);
	let record = scope
		.insert(Creation {
			id,
			area: request.area,
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
	match scope.update(&mut record).await {
		// The approval changed since it was loaded: another call consumed it,
		// or an Evidence Revocation closed it. Either wins the race.
		Err(Error::Conflict(_)) => Ok(false),
		other => other.map(|()| true),
	}
}

/// Admit one exact call of a Tenant's HTTP or MCP tool. Policy may permit it
/// directly (`tool.call_unapproved`) or through a Run grant; otherwise a
/// designated approver decides through the ordinary approval decision and the
/// Run waits on the returned approval. The same invocation key and call always
/// map to the same approval, which the one call it allows consumes.
pub async fn tool_call(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	request: ToolCallRequest,
) -> Result<ToolCallApproval> {
	let limits = scope.limits()?;
	if run.phase.is_terminal() || run.control == RunControl::Cancelled {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let digest = aidash_domain::registry::rules::digest(&json!([
		TOOL_CALL,
		run.id,
		request.key,
		request.call
	]));
	let cache_key = derived_key(json!([TOOL_CALL, digest]));
	if let Some(previous) = scope.cached(cache_key, &digest).await? {
		let id = serde_json::from_value(previous["operation_id"].clone())?;
		let record = scope.load(id, TOOL_CALL).await?;
		return recorded_tool_call(scope, run, record).await;
	}
	let destination = tool_target(&request.tool, &request.operation);
	let target = tool_resource(scope, &request.tool, &request.operation, run.id);
	let direct = match ordinary(scope, &target, CALL_UNAPPROVED) {
		Err(Error::Forbidden) => return Ok(ToolCallApproval::Refused(Refusal::Forbidden)),
		other => other?,
	};
	// A Read-only operation needs no approval and leaves no record; only the
	// Tenant's policy can still forbid the call.
	if request.read_only {
		return Ok(ToolCallApproval::Approved { approval_id: None });
	}
	// Unlike a Web disclosure's, a tool grant outlives new user input: the tool's
	// destination is fixed by the Tenant, not chosen from the conversation.
	let grant = scope.grants(run.id).await?.into_iter().find(|g| {
		grants(g, CALL_UNAPPROVED)
			&& g.data["targets"]
				.as_array()
				.is_some_and(|a| a.contains(&json!(destination)))
			&& g.data["subjects"] == json!(scope.subjects())
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
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		let name = tool_rule_target(&request.tool.id, &request.operation);
		advise(scope, run, ApprovalKind::ToolCall, &name, &target).await?
	};
	let state = approvals::requested_state(direct, grant.is_some(), designated.is_some());
	let state = advice.state("approved").unwrap_or(state);
	let id = Uuid::new_v4();
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),
		"tool":request.tool.id,"version":request.tool.version,"operation":request.operation,"call":request.call,"invocation_key":request.key,"targets":[destination],"action":CALL_UNAPPROVED,
		"digest":digest,"policy_revision":scope.policy_revision(),"approver":designated,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	advice.record(&mut data);
	let record = scope
		.insert(Creation {
			id,
			area: request.area,
			kind: TOOL_CALL,
			// A direct or granted call is consumed by this call.
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
		.cache(cache_key, &digest, &json!({"operation_id":id}))
		.await?;
	scope.event(run.workspace_id,"capability.approval_requested",json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"],"approver":record.data["approver"],"action":CALL_UNAPPROVED})).await?;
	Ok(settled_tool_call(&record))
}

/// Why a denied approval refuses its call: a person's rule or mode, the approval
/// Decider, or its approver.
fn refusal_of(record: &Record) -> Refusal {
	let by = &record.data["refused_by"];
	if by.get("rule").is_some() {
		Refusal::Rule
	} else if by["mode"] == "dont_ask" {
		Refusal::DontAsk
	} else if by["mode"] == "auto" {
		serde_json::from_value(by["reason"].clone()).map_or(Refusal::Denied, Refusal::Auto)
	} else {
		Refusal::Denied
	}
}

/// What a recorded approval says about its call, once nothing is left to decide
/// or consume. An `approved` record is not settled: see [`recorded_tool_call`].
fn settled_tool_call(record: &Record) -> ToolCallApproval {
	match (record.state.as_str(), record.expires_at) {
		("consumed", _) => ToolCallApproval::Approved {
			approval_id: (record.data["ordinary"] != true).then_some(record.id),
		},
		("pending", Some(expires_at)) if expires_at > Utc::now() => ToolCallApproval::Pending {
			approval_id: record.id,
			expires_at,
		},
		("pending", _) => ToolCallApproval::Refused(Refusal::Expired),
		("denied", _) => ToolCallApproval::Refused(refusal_of(record)),
		("revoked", _) => ToolCallApproval::Refused(Refusal::Revoked),
		("closed", _) => ToolCallApproval::Refused(Refusal::Closed),
		_ => ToolCallApproval::Refused(Refusal::Blocked),
	}
}

/// Map the approval recorded for a call the Run asks about again. An approved
/// one is consumed here, by the one call it allows, unless it has lapsed since
/// the decision.
async fn recorded_tool_call(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	mut record: Record,
) -> Result<ToolCallApproval> {
	if record.state != "approved" {
		return Ok(settled_tool_call(&record));
	}
	if !still_approved(scope, run, &record).await? {
		let expired = record.expires_at.is_none_or(|t| t <= Utc::now());
		return Ok(ToolCallApproval::Refused(if expired {
			Refusal::Expired
		} else {
			Refusal::Revoked
		}));
	}
	record.state = "consumed".into();
	record.data["consumed_at"] = json!(Utc::now());
	match scope.update(&mut record).await {
		Ok(()) => Ok(settled_tool_call(&record)),
		// The approval changed since it was loaded: an Evidence Revocation closed
		// it or the approver revoked it. That wins the race.
		Err(Error::Conflict(_)) => {
			let current = scope.load(record.id, TOOL_CALL).await?;
			Ok(settled_tool_call(&current))
		}
		Err(error) => Err(error),
	}
}

/// Whether the author of a Run input is a person: an enabled User subject of the
/// Tenant's policy. A label another Node's agent or a Federation message
/// carries, such as `human`, names no subject, so it is never a person here.
fn is_person(bundle: &PolicyBundle, author: &str) -> bool {
	bundle
		.subjects
		.get(author)
		.is_some_and(|subject| subject.enabled && subject.kind == SubjectKind::User)
}

/// Whether the person's Auto mode still leaves `record` to the approval Decider,
/// as the Tenant's policy, the person's rules and the Run's blocks stand now.
async fn judged_now(scope: &mut dyn ApprovalScope, run: &Run, record: &Record) -> Result<bool> {
	let (kind, name) = rule_of(record)?;
	let destination = record.data["targets"][0].as_str().ok_or(Error::Forbidden)?;
	let (target, _) = target_of(scope, record, destination, run.id);
	Ok(matches!(
		advise(scope, run, kind, &name, &target).await?,
		Advice::Judge
	))
}

/// The pending approval of `run` that the approval Decider is to judge, with
/// what the Decider is told about who asked. `None` for anything else, so the
/// Run waits for a person as it does in Manual.
pub async fn judgement(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	id: Uuid,
) -> Result<Option<JudgeRequest>> {
	let record = match load_approval(scope, id).await {
		Err(Error::NotFound(_)) => return Ok(None),
		other => other?,
	};
	if record.data["run_id"] != json!(run.id)
		|| record.data["judge"] != true
		|| record.state != "pending"
		|| record.expires_at.is_none_or(|t| t <= Utc::now())
		|| !judged_now(scope, run, &record).await?
	{
		return Ok(None);
	}
	let (kind, _) = rule_of(&record)?;
	let target = record.data["targets"][0]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	let inputs = scope.run_inputs(run).await?;
	let bundle = scope.bundle();
	let people = inputs
		.into_iter()
		.filter(|input| is_person(bundle, &input.sender))
		.collect();
	Ok(Some(JudgeRequest {
		kind,
		target,
		people,
	}))
}

/// Settle a judged approval as the approval Decider decided, in the name of the
/// person the Run's chain starts at. Nothing changes when a person or an earlier
/// settlement already decided it, or, for an allowance, when the person's Auto
/// mode no longer covers it.
pub async fn settle(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	id: Uuid,
	settlement: Settlement,
) -> Result<()> {
	let mut record = load_approval(scope, id).await?;
	if record.data["run_id"] != json!(run.id) || record.data["judge"] != true {
		return Err(Error::NotFound("approval unavailable".into()));
	}
	let person = scope.chain_root().ok_or(Error::Forbidden)?.to_owned();
	let (advice, state, evidence) = match settlement {
		Settlement::Shadow { .. } => return Ok(()),
		Settlement::Allowed { evidence } => (
			Advice::Approve {
				by: GivenBy::Auto(evidence),
				person,
			},
			"approved",
			evidence,
		),
		Settlement::Blocked { reason, evidence } => (
			Advice::Refuse {
				by: RefusedBy::Auto(reason),
				person,
			},
			"denied",
			evidence,
		),
	};
	if record.state != "pending"
		|| matches!(settlement, Settlement::Allowed { .. })
			&& !judged_now(scope, run, &record).await?
	{
		return Ok(());
	}
	advice.record(&mut record.data);
	record.data["decision"] = json!(evidence);
	record.state = state.into();
	match scope.update(&mut record).await {
		Ok(()) => {}
		// A person decided it, or its Run ended, since it was loaded.
		Err(Error::Conflict(_)) => return Ok(()),
		Err(error) => return Err(error),
	}
	let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":Value::Null});
	scope
		.event(run.workspace_id, "capability.approval_decided", result)
		.await
}

#[cfg(test)]
mod tests;

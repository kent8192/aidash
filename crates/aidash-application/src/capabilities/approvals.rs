//! Approval requests, human decisions and worker authorization retain one live scope.
pub use crate::ports::capabilities::approvals::{
	JudgeRequest, Refusal, SamplingApprovalRequest, Settlement, ToolCallApproval, ToolCallRequest,
	ToolCallTarget,
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
	policy::{Evaluation, PolicyBundle, Resource, SubjectKind},
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
pub const KINDS: [&str; 3] = [OUTBOUND, TOOL_CALL, SAMPLING];
pub const SAMPLING: &str = "sampling";
pub const SAMPLE_UNAPPROVED: &str = "sampling.request_unapproved";
pub mod sampling;
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
/// The shortest Run grant an approver may give, and so the least a
/// Deployment's `grant_seconds` may be. The dashboard offers whole minutes;
/// a few seconds of slack let a one-minute grant survive the trip to the
/// server.
pub const MIN_RUN_GRANT_SECONDS: i64 = 60;
const GRANT_CLOCK_SLACK_SECONDS: i64 = 5;

/// The target string of an approval record and Run grant for one operation of
/// a tool. It cannot collide with an outbound origin. Page reading names the
/// host it contacts, so a Run grant covers that one host (#987).
pub fn tool_target(tool: &ToolCallTarget, operation: &str, host: Option<&str>) -> String {
	match host {
		Some(host) => format!("tool:{}@{}#{operation}/{host}", tool.id(), tool.version()),
		None => format!("tool:{}@{}#{operation}", tool.id(), tool.version()),
	}
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

/// A person's approval authority is evaluated as an API decision, including
/// when a worker applies their saved rules or mode.
fn api_evaluation(
	scope: &dyn ApprovalScope,
	subject: &str,
	target: &Resource,
	action: &str,
) -> Evaluation {
	let mut evaluation = scope.evaluation(subject, target, action);
	evaluation.environment["transport"] = json!("api");
	evaluation
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
			.evaluate(&api_evaluation(scope, root, resource, action))
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
	let mut settings = scope.approval_settings(run).await?;
	if kind == ApprovalKind::Sampling
		&& let Some(settings) = &mut settings
		&& let Some(raw) = name.strip_prefix("sampling:")
		&& let Ok(parts) = serde_json::from_str::<Vec<Value>>(raw)
		&& parts.len() == 5
	{
		let deny = format!("sampling:{}", json!(&parts[..3]));
		for rule in &mut settings.rules {
			if rule.kind == ApprovalKind::Sampling
				&& rule.effect == RuleEffect::Deny
				&& rule.target == RuleTarget::Exact(deny.clone())
			{
				rule.target = RuleTarget::Exact(name.into());
			}
		}
	}
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
			Resolution::Judge if kind == ApprovalKind::Sampling => Advice::Ask,
			Resolution::Judge => Advice::Judge,
			Resolution::Ask | Resolution::Proceed => Advice::Ask,
		},
	)
}
/// The person's current mode after applying the same policy ceiling as approvals.
pub async fn connection_mode(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	target: &Resource,
) -> Result<Option<ApprovalMode>> {
	let Some(settings) = scope.approval_settings(run).await? else {
		return Ok(None);
	};
	let Some(root) = scope.chain_root() else {
		return Ok(None);
	};
	let allowed = ceiling(scope, &settings, root, target).mode_allowed;
	let suspended =
		settings.mode == ApprovalMode::Auto && scope.auto_blocks(run).await?.suspended();
	Ok(Some(if allowed && !suspended {
		settings.mode
	} else {
		ApprovalMode::Manual
	}))
}

/// Whether the Run's effective Approval Mode is Auto: the person selected it,
/// the policy allows it, the Decider is available, and its block limits have
/// not suspended it. The model is told not to stop for clarifying questions
/// only while this holds.
pub async fn auto_judging(scope: &mut dyn ApprovalScope, run: &Run) -> Result<bool> {
	let Some(settings) = scope.approval_settings(run).await? else {
		return Ok(false);
	};
	if settings.mode != ApprovalMode::Auto {
		return Ok(false);
	}
	let Some(root) = scope.chain_root() else {
		return Ok(false);
	};
	let mode_allowed = settings
		.mode
		.policy_requirement()
		.is_none_or(|(action, id)| {
			scope
				.bundle()
				.evaluate(&api_evaluation(
					scope,
					root,
					&scope.resource(MODE_RESOURCE, id, json!({})),
					action,
				))
				.allowed
		});
	let ceiling = settings.ceiling(Standing {
		approver: false,
		advance: false,
		mode: mode_allowed,
	});
	Ok(ceiling.mode_allowed && !scope.auto_blocks(run).await?.suspended())
}
fn tool_resource(
	scope: &dyn ApprovalScope,
	tool: &ToolCallTarget,
	operation: &str,
	run: Uuid,
) -> Resource {
	let mut attributes = json!({"version":tool.version(),"operation":operation,"run_id":run,"action":CALL_UNAPPROVED});
	if let ToolCallTarget::AgentPlugin(plugin) = tool {
		attributes["plugin"] = json!(plugin.plugin);
		attributes["server"] = json!(plugin.server);
		attributes["plugin_tool"] = json!(plugin.tool);
	}
	scope.resource("tool", &tool.id(), attributes)
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
) -> Result<(Resource, &'static str)> {
	// A Run grant from an "allow for this Run" decision names the action it
	// covers; it protects the same resource as the approval it came from.
	Ok(
		if record.kind == SAMPLING || grants(record, SAMPLE_UNAPPROVED) {
			let target: aidash_domain::agent_plugin::interaction::SamplingTarget =
				serde_json::from_value(record.data["sampling_target"].clone())?;
			(sampling::resource(scope, &target, run), SAMPLE_UNAPPROVED)
		} else if record.kind == TOOL_CALL || grants(record, CALL_UNAPPROVED) {
			let tool = ToolCallTarget::from_record(&record.data)?;
			let operation = record.data["operation"].as_str().unwrap_or_default();
			(tool_resource(scope, &tool, operation, run), CALL_UNAPPROVED)
		} else {
			(resource(scope, target, run), "network.get")
		},
	)
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
	let evaluation = api_evaluation(scope, subject, target, "capability.approve");
	approvals::approver_eligible(scope.bundle(), &evaluation, requester)
}
/// Whether anyone may decide an approval of `target` that the current
/// principal requests: they themselves, or another Eligible approver.
fn any_approver(scope: &dyn ApprovalScope, target: &Resource) -> bool {
	let requester = scope.principal();
	eligible(scope, requester, requester, target)
		|| scope
			.bundle()
			.subjects
			.keys()
			.any(|id| eligible(scope, id, requester, target))
}

/// The protected targets of `record`, with its Run's context loaded.
async fn targets_of(
	scope: &mut dyn ApprovalScope,
	record: &Record,
) -> Result<Option<Vec<Resource>>> {
	let run: Uuid = serde_json::from_value(record.data["run_id"].clone())?;
	scope.run_context(run).await?;
	let Some(targets) = record.data["targets"]
		.as_array()
		.filter(|items| !items.is_empty())
	else {
		return Ok(None);
	};
	let mut resources = Vec::with_capacity(targets.len());
	for origin in targets {
		let Some(origin) = origin.as_str() else {
			return Ok(None);
		};
		resources.push(target_of(scope, record, origin, run)?.0);
	}
	Ok(Some(resources))
}

/// Whether `subject` is an Eligible approver of every target, as the Tenant's
/// policy stands now.
fn eligible_for_all(
	scope: &dyn ApprovalScope,
	subject: &str,
	requester: &str,
	targets: &[Resource],
) -> bool {
	targets
		.iter()
		.all(|target| eligible(scope, subject, requester, target))
}

/// Who may decide a pending approval, judged now (ADR 0135): its requester
/// alone when they are an Eligible approver of it, otherwise every Eligible
/// approver. Nothing about whom it was addressed to is stored.
fn decides(
	scope: &dyn ApprovalScope,
	subject: &str,
	requester: &str,
	targets: &[Resource],
) -> bool {
	if eligible_for_all(scope, requester, requester, targets) {
		subject == requester
	} else {
		eligible_for_all(scope, subject, requester, targets)
	}
}

/// What the current principal may do with an approval record or Run grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reader {
	/// They may see it: its requester, or someone it concerns as an approver.
	pub visible: bool,
	/// They act on it as an approver: they may decide it while it is pending;
	/// for a Run grant they may approve its kind for its destination now; for
	/// any other approval they decided or gave it and still may approve it.
	pub approver: bool,
}

/// What the current principal may do with `record`, judged now.
pub async fn reader(scope: &mut dyn ApprovalScope, record: &Record) -> Result<Reader> {
	let principal = scope.principal().to_owned();
	let owner = record.owner == principal;
	// A record outlives a Thread that is no longer live, as on a
	// soft-deleted Node. Nobody can act on it as an approver then, but its
	// requester still sees it, and one such record must not fail a whole list.
	let targets = match targets_of(scope, record).await {
		Err(Error::NotFound(_) | Error::Forbidden) => None,
		other => other?,
	};
	let Some(targets) = targets else {
		return Ok(Reader {
			visible: owner,
			approver: false,
		});
	};
	let approver = if record.kind == "grant" {
		// Anyone who may approve its kind for its destination now may revoke a
		// Run grant (ADR 0135).
		eligible_for_all(scope, &principal, &record.owner, &targets)
	} else if record.state == "pending" {
		decides(scope, &principal, &record.owner, &targets)
	} else {
		// A decided approval leaves the lists of the approvers who did not
		// decide it (ADR 0135); only who decided or gave it keeps it, while
		// they may still approve it, as before.
		let gave = ["decided_by", "approver"]
			.iter()
			.any(|field| record.data[*field].as_str() == Some(principal.as_str()));
		gave && eligible_for_all(scope, &principal, &record.owner, &targets)
	};
	Ok(Reader {
		visible: owner || approver,
		approver,
	})
}

pub async fn visible(scope: &mut dyn ApprovalScope, record: &Record) -> Result<bool> {
	if record.owner == scope.principal() {
		return Ok(true);
	}
	Ok(reader(scope, record).await?.visible)
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
	if run.phase.is_terminal() || run.control != RunControl::Active {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let target = resource(scope, &origin, run.id);
	scope.require(&target, "capability.request").await?;
	let direct = ordinary(scope, &target, "network.get")?;
	let grants = scope.grants(run.id).await?;
	let grant = grants.into_iter().find(|g| {
		g.data["targets"]
			.as_array()
			.is_some_and(|a| a.contains(&json!(origin)))
			// Outbound reads and tool calls never share a grant.
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
	// `approver` names who gave the approval, once someone has; a request
	// waiting for an Eligible approver names nobody (ADR 0135).
	let given = grant
		.as_ref()
		.and_then(|g| g.data["approver"].as_str().map(str::to_owned));
	let approvable = given.is_some() || any_approver(scope, &target);
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		advise(scope, run, ApprovalKind::Outbound, &origin, &target).await?
	};
	let state = approvals::requested_state(direct, grant.is_some(), approvable);
	let state = advice.state("approved").unwrap_or(state);
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),"url":input.url,"targets":[origin],"action":"network.get","digest":digest,"policy_revision":scope.policy_revision(),"approver":given,"grant_id":grant.map(|g|g.id),"ordinary":direct});
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
	scope
		.event(
			run.thread_id,
			"capability.approval_requested",
			json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"]}),
		)
		.await?;
	view(scope, run, &record).await
}
/// The state of an outbound read that waits for the Home's person (ADR 0139, decision 22). The
/// receiving Node's people cannot decide it, and no worker takes it: only the Home's answer
/// moves it to `approved` or `denied`.
pub const AWAITING_HOME: &str = "awaiting_home";
/// What the Home's person is asked about an outbound read of a Run executing for another Home.
pub struct HomeAsk {
	pub record: Uuid,
	/// The Human Request already put to the Home's person, when one was.
	pub home_request: Option<Uuid>,
	/// The prompt to put to them when none was.
	pub prompt: String,
	/// The command key the Home deduplicates the request under.
	pub key: String,
}
pub enum HomeOutbound {
	/// The call's result: a repeat of one that was settled, or the view of a waiting record.
	Settled(Value),
	/// The Home's person must be asked, or the answer read.
	Ask(HomeAsk),
}
/// What the Home reported about the request put to its person.
pub enum HomeAnswer {
	/// The Home created this request.
	Asked(Uuid),
	/// The Home's current state of this request.
	Read {
		request: Uuid,
		verdict: aidash_domain::capabilities::approvals::HomeVerdict,
	},
}
/// First step of `outbound_get` for a Run executing for another Home: the receiving Node's own
/// checks (its allowed origins, its admission, its policy on the request) and a record of the
/// read that is `awaiting_home`. Asking the Home's person is the caller's, outside the
/// transaction. A repeat of the call finds its record by key, so a retry never asks twice.
pub async fn home_begin(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	area: Uuid,
	input: Outbound,
) -> Result<HomeOutbound> {
	let limits = scope.limits()?;
	let (_, origin) = permitted_origin(&input.url, &limits.origins)?;
	let digest = aidash_domain::registry::rules::digest(&json!(["outbound_get", run.id, input]));
	let key = format!("run:{}:outbound:{digest}", run.id);
	if let Some(previous) = scope.cached(input.idempotency_key, &digest).await? {
		let id: Uuid = serde_json::from_value(previous["operation_id"].clone())?;
		let record = scope.load(id, OUTBOUND).await?;
		if record.state == AWAITING_HOME {
			return Ok(HomeOutbound::Ask(HomeAsk {
				record: id,
				home_request: serde_json::from_value::<Option<Uuid>>(
					record.data["home_request_id"].clone(),
				)
				.ok()
				.flatten(),
				prompt: aidash_domain::capabilities::approvals::outbound_prompt(
					&input.url, &origin,
				),
				key,
			}));
		}
		return view(scope, run, &record).await.map(HomeOutbound::Settled);
	}
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if run.phase.is_terminal() || run.control != RunControl::Active {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	// The receiving Node's own policy on the request stays: the Home's person adds to it and
	// never replaces it.
	let target = resource(scope, &origin, run.id);
	scope.require(&target, "capability.request").await?;
	let id = Uuid::new_v4();
	let data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),"url":input.url,"targets":[origin],"action":"network.get","digest":digest,"policy_revision":scope.policy_revision(),"approver":Value::Null,"grant_id":Value::Null,"ordinary":false,"approval_mode":aidash_domain::capabilities::approvals::HOME_APPROVAL_MODE});
	scope
		.insert(Creation {
			id,
			area: Some(area),
			kind: OUTBOUND,
			state: AWAITING_HOME,
			data,
			expires: Some(Utc::now() + Duration::seconds(limits.approval_seconds as i64)),
		})
		.await?;
	scope
		.cache(input.idempotency_key, &digest, &json!({"operation_id":id}))
		.await?;
	Ok(HomeOutbound::Ask(HomeAsk {
		record: id,
		home_request: None,
		prompt: aidash_domain::capabilities::approvals::outbound_prompt(&input.url, &origin),
		key,
	}))
}
/// The view of a read that waits for the Home's person looks like one waiting for a person here.
fn home_waiting(mut view: Value) -> Value {
	view["status"] = json!("approval_required");
	view["approval_state"] = json!(AWAITING_HOME);
	view["approver"] = json!("home");
	view
}
/// Second step: record what the Home answered. An approval makes the record `approved`, which
/// the outbound worker takes, and which the worker checks against this Node's own authority and
/// origins as it fetches. A decline, an expiry, or anything that is not an explicit approval
/// makes it `denied`, the shape a person's denial has here.
pub async fn home_settle(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	record: Uuid,
	answer: HomeAnswer,
) -> Result<Value> {
	use aidash_domain::capabilities::approvals::HomeVerdict;
	let mut record = scope.load(record, OUTBOUND).await?;
	if record.state != AWAITING_HOME {
		return view(scope, run, &record).await;
	}
	if record.data["run_id"] != json!(run.id) {
		return Err(Error::NotFound("operation unavailable".into()));
	}
	let verdict = match answer {
		HomeAnswer::Asked(request) => {
			if record.data["home_request_id"].is_null() {
				record.data["home_request_id"] = json!(request);
				scope.update(&mut record).await?;
			}
			HomeVerdict::Pending
		}
		HomeAnswer::Read { request, verdict } => {
			if record.data["home_request_id"] != json!(request) {
				return Err(Error::Forbidden);
			}
			verdict
		}
	};
	let lapsed = record.expires_at.is_none_or(|at| at <= Utc::now());
	let verdict = match verdict {
		HomeVerdict::Pending if lapsed => HomeVerdict::Expired,
		other => other,
	};
	match verdict {
		HomeVerdict::Pending => {
			let projected = view(scope, run, &record).await?;
			return Ok(home_waiting(projected));
		}
		HomeVerdict::Approved { by } => {
			if run.control == RunControl::Cancelled || run.phase.is_terminal() {
				return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
			}
			// Origins are the receiving Node's to allow, and can change while the Home thinks.
			let limits = scope.limits()?;
			permitted_origin(
				record.data["url"].as_str().ok_or(Error::Forbidden)?,
				&limits.origins,
			)?;
			record.state = "approved".into();
			record.data["approver"] = json!(by.as_deref().unwrap_or("home"));
			record.data["decided_by"] = json!(by.as_deref().unwrap_or("home"));
		}
		HomeVerdict::Declined => {
			record.state = "denied".into();
			record.data["error"] = json!("OUTBOUND_HOME_DECLINED");
			record.data["decided_by"] = json!("home");
		}
		HomeVerdict::Expired => {
			record.state = "denied".into();
			record.data["error"] = json!("OUTBOUND_HOME_EXPIRED");
			record.data["decided_by"] = json!("home");
		}
	}
	record.data["decided_at"] = json!(Utc::now());
	scope.update(&mut record).await?;
	view(scope, run, &record).await
}
pub async fn view(scope: &mut dyn ApprovalScope, run: &Run, record: &Record) -> Result<Value> {
	if record.owner != scope.principal() || record.data["run_id"] != json!(run.id) {
		return Err(Error::NotFound("operation unavailable".into()));
	}
	let expired = record.expires_at.is_some_and(|t| t < Utc::now());
	let mut result = approvals::projection(record, expired);
	if matches!(record.kind.as_str(), TOOL_CALL | SAMPLING) {
		// What the approver is asked to allow: the exact call.
		for field in [
			"tool",
			"version",
			"plugin_tool",
			"operation",
			"call",
			"sampling_target",
			"sampling",
		] {
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
		SAMPLING => {
			let target: aidash_domain::agent_plugin::interaction::SamplingTarget =
				serde_json::from_value(data["sampling_target"].clone())?;
			(ApprovalKind::Sampling, target.allow_target())
		}
		OUTBOUND => (ApprovalKind::Outbound, text(&data["targets"][0])?),
		_ => (
			ApprovalKind::ToolCall,
			tool_rule_target(&text(&data["tool"])?, &text(&data["operation"])?),
		),
	})
}
/// Approval records of every kind share one decision API.
async fn load_approval(scope: &mut dyn ApprovalScope, id: Uuid) -> Result<Record> {
	for kind in KINDS {
		match scope.load(id, kind).await {
			Err(Error::NotFound(_)) => {}
			other => return other,
		}
	}
	Err(Error::NotFound("approval unavailable".into()))
}
/// Whether the current reader can turn this pending approval into an allow
/// rule. The dashboard and decision path share the live advance ceiling.
pub async fn can_allow_always(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	record: &Record,
) -> Result<bool> {
	let owner = scope.principal().to_owned();
	if record.state != "pending"
		|| !KINDS.contains(&record.kind.as_str())
		|| record.data["subjects"][0].as_str() != Some(owner.as_str())
		// Only a requester deciding their own approval; the ceiling below
		// checks they are an Eligible approver of it.
		|| record.owner != owner
		|| record.data["run_id"] != json!(run.id)
		|| record.expires_at.is_none_or(|t| t <= Utc::now())
		|| run.control == RunControl::Cancelled
		|| run.phase.is_terminal()
	{
		return Ok(false);
	}
	let Some(origin) = record.data["targets"][0].as_str() else {
		return Ok(false);
	};
	let (target, _) = target_of(scope, record, origin, run.id)?;
	let Some(settings) = scope.approval_settings(run).await? else {
		return Ok(false);
	};
	Ok(ceiling(scope, &settings, &owner, &target).may_approve)
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
	let (target, action) = target_of(scope, &record, origin, run_id)?;
	let principal = scope.principal().to_owned();
	let targets = targets_of(scope, &record)
		.await?
		.ok_or_else(|| Error::NotFound("approval unavailable".into()))?;
	if !decides(scope, &principal, &record.owner, &targets) {
		return Err(Error::NotFound("approval unavailable".into()));
	}
	// Every Eligible approver is offered it and the first decision counts
	// (ADR 0135). A later one is told who decided and how; only a person who
	// may decide it now gets this far. Every state after a decision counts,
	// including an allowed outbound read its worker has since completed or left
	// uncertain, and one an Approval Rule, mode or the Auto Decider settled.
	if let Some(by) = record.data["decided_by"]
		.as_str()
		.filter(|_| record.state != "pending")
	{
		return Err(Error::ApprovalDecided {
			decided_by: by.into(),
			outcome: if record.state == "denied" {
				approvals::DecidedOutcome::Denied
			} else {
				approvals::DecidedOutcome::Allowed
			},
		});
	}
	if record.revision != input.expected_revision
		|| record.state != "pending"
		|| record.expires_at.is_none_or(|t| t <= Utc::now())
	{
		return Err(Error::Conflict("APPROVAL_STALE_OR_EXPIRED".into()));
	}
	if run.control == RunControl::Cancelled || run.phase.is_terminal() {
		// A tool call approval ends with its Run. One its Run's
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
			// A rule approves in advance only within the ceiling `resolve`
			// applies to it. Without `approval.advance` on this resource the
			// rule would be stored and ignored, so "Always allow" cannot mean
			// what it says: refuse it and leave "Allow once" available.
			if !can_allow_always(scope, &run, &record).await? {
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
			// A tool grant outlives new user input unless the call named the
			// context revision it was made at, as page reading does (#987).
			if record.kind == SAMPLING {
				data["sampling_target"] = record.data["sampling_target"].clone();
			} else if record.kind == TOOL_CALL {
				data["tool"] = record.data["tool"].clone();
				data["version"] = record.data["version"].clone();
				data["plugin_tool"] = record.data["plugin_tool"].clone();
				if !record.data["context_revision"].is_null() {
					data["context_revision"] = record.data["context_revision"].clone();
				}
			} else {
				data["context_revision"] = record.data["context_revision"].clone();
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
	// Whoever decides is the approver the decision rests on: later checks
	// require them to stay an Eligible approver.
	record.data["approver"] = json!(scope.principal());
	record.data["decided_at"] = json!(Utc::now());
	record.data["decision_policy_revision"] = json!(scope.policy_revision());
	scope.update(&mut record).await?;
	let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":record.data["grant_id"]});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	scope
		.event(run.thread_id, "capability.approval_decided", result.clone())
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
	// Its requester or whoever gave it may revoke it; so may anyone who may
	// approve its kind for its destination now, for a Run grant (ADR 0135).
	if record.owner != scope.principal()
		&& record.data["approver"] != scope.principal()
		&& !reader(scope, &record).await?.approver
	{
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
	// An approval the Home's person gave stands on the Home's authority, which this Node's policy
	// does not know, so there is no approver to judge here. Everything of this Node's own is still
	// judged above: its origins, `capability.request` and any explicit deny of `network.get`.
	let home_approved =
		record.data["approval_mode"] == aidash_domain::capabilities::approvals::HOME_APPROVAL_MODE;
	if !direct && !home_approved {
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
/// Whether an approved tool call of `run` still admits what it
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
	let (target, action) = target_of(scope, record, destination, run.id)?;
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

/// Admit one exact call of a Tenant's HTTP or MCP tool. Policy may permit it
/// directly (`tool.call_unapproved`) or through a Run grant; otherwise a
/// Eligible approver decides through the ordinary approval decision and the
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
	let destination = tool_target(&request.tool, &request.operation, request.host.as_deref());
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
	// A tool grant outlives new user input: the tool's destination is fixed by
	// the Tenant, not chosen from the conversation. Page reading's host is
	// chosen from it, so its grant ends at the next user input (#987).
	let grant = scope.grants(run.id).await?.into_iter().find(|g| {
		grants(g, CALL_UNAPPROVED)
			&& g.data["targets"]
				.as_array()
				.is_some_and(|a| a.contains(&json!(destination)))
			&& request
				.context_revision
				.is_none_or(|revision| g.data["context_revision"] == json!(revision))
			&& g.data["subjects"] == json!(scope.subjects())
			&& eligible(
				scope,
				g.data["approver"].as_str().unwrap_or_default(),
				&g.owner,
				&target,
			)
	});
	// `approver` names who gave the approval, once someone has (ADR 0135).
	let given = grant
		.as_ref()
		.and_then(|g| g.data["approver"].as_str().map(str::to_owned));
	let approvable = given.is_some() || any_approver(scope, &target);
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		let name = tool_rule_target(&request.tool.id(), &request.operation);
		advise(scope, run, ApprovalKind::ToolCall, &name, &target).await?
	};
	let state = approvals::requested_state(direct, grant.is_some(), approvable);
	let state = advice.state("approved").unwrap_or(state);
	let id = Uuid::new_v4();
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),
		"operation":request.operation,"call":request.call,"invocation_key":request.key,"targets":[destination],"action":CALL_UNAPPROVED,"context_revision":request.context_revision,
		"digest":digest,"policy_revision":scope.policy_revision(),"approver":given,"grant_id":grant.map(|g|g.id),"ordinary":direct});
	if let (Some(data), Some(target)) = (data.as_object_mut(), request.tool.record().as_object()) {
		data.extend(target.clone());
	}
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
	scope.event(run.thread_id,"capability.approval_requested",json!({"id":id,"run_id":run.id,"status":state,"targets":record.data["targets"],"action":CALL_UNAPPROVED})).await?;
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
		// The approval changed since it was loaded: it was closed meanwhile
		// or the approver revoked it. That wins the race.
		Err(Error::Conflict(_)) => {
			let current = scope.load(record.id, &record.kind).await?;
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
	let (target, _) = target_of(scope, record, destination, run.id)?;
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
		.filter(|input| input.seq <= run.observed_input_seq)
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
) -> Result<bool> {
	let mut record = load_approval(scope, id).await?;
	if record.data["run_id"] != json!(run.id) || record.data["judge"] != true {
		return Err(Error::NotFound("approval unavailable".into()));
	}
	let person = scope.chain_root().ok_or(Error::Forbidden)?.to_owned();
	let (advice, state, evidence) = match settlement {
		Settlement::Shadow { .. } => return Ok(false),
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
	if record.state != "pending" || !judged_now(scope, run, &record).await? {
		return Ok(false);
	}
	advice.record(&mut record.data);
	record.data["decision"] = json!(evidence);
	record.state = state.into();
	match scope.update(&mut record).await {
		Ok(()) => {}
		// A person decided it, or its Run ended, since it was loaded.
		Err(Error::Conflict(_)) => return Ok(false),
		Err(error) => return Err(error),
	}
	let result = json!({"approval_id":id,"state":record.state,"revision":record.revision,"grant_id":Value::Null});
	scope
		.event(run.thread_id, "capability.approval_decided", result)
		.await?;
	Ok(true)
}

#[cfg(test)]
mod tests;

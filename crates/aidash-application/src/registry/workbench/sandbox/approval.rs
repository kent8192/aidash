//! The starter of a Workbench test answers the approvals its real calls need
//! (ADR 0127). The rule is decided here once. Offering a request, waiting for
//! the answer, answering it, showing it and rechecking it before dispatch all
//! use [`standing`]. The server keeps only persistence.
//!
//! Every path locks the actor's identity and Tenant policy before the session.
use crate::{
	Error, Result,
	ports::registry::workbench::sandbox::{
		Approver, SandboxRepository, SandboxScope, dispatch::RealDispatchRepository,
		execution::TestApproval,
	},
};
use aidash_domain::{
	capabilities::approvals::{workbench_approver_eligible, workbench_policy},
	policy::{Evaluation, PolicyBundle},
	registry::workbench::{Draft, sandbox::TestSession},
};
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// The only actions a test approval can name.
const ACTIONS: [&str; 3] = ["tool.call_unapproved", "network.get", "research.disclose"];
/// How often a waiting test reads its starter's answer.
const POLL: Duration = Duration::from_millis(500);

/// What the current policy says about one approval for one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
	/// The call needs no answer.
	Allowed,
	/// Only the starter may answer, and they may now.
	Askable,
	/// Nobody may answer, so the call is refused.
	Refused,
}

/// The starter's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
	AllowOnce,
	Deny,
}

/// The single rule. Whether `approver` may have `approval` in `session` now,
/// without a Run or workspace.
pub fn standing(
	bundle: &PolicyBundle,
	approver: Option<&Approver>,
	session: &TestSession,
	approval: &TestApproval,
	outbound_admitted: bool,
) -> Standing {
	let Some(approver) = approver else {
		return Standing::Refused;
	};
	if !ACTIONS.contains(&approval.action)
		|| (approval.action == "network.get" && !outbound_admitted)
	{
		return Standing::Refused;
	}
	let direct = workbench_policy(bundle).evaluate(&Evaluation {
		subject: approver.subject.clone(),
		action: approval.action.into(),
		resource: approval.resource.clone(),
		environment: json!({}),
	});
	if direct.allowed || (direct.reason == "no_matching_allow" && !approval.requires_approval) {
		return Standing::Allowed;
	}
	let starter = approver.person.is_some_and(|person| {
		session.scenario["starter_identity"] == json!(person)
			&& session.scenario["starter_subject"] == approver.subject
	});
	if direct.reason == "no_matching_allow"
		&& starter
		&& workbench_approver_eligible(bundle, &approver.subject, &approval.resource)
	{
		Standing::Askable
	} else {
		Standing::Refused
	}
}

/// The approval a stored request names, if it names a known action.
fn requested(pending: &Value) -> Option<TestApproval> {
	Some(TestApproval {
		action: ACTIONS
			.into_iter()
			.find(|action| pending["action"] == *action)?,
		resource: serde_json::from_value(pending["resource"].clone()).ok()?,
		call: serde_json::from_value(pending["call"].clone()).ok()?,
		requires_approval: true,
	})
}

struct Judged {
	session: TestSession,
	standing: Standing,
}

async fn judge(scope: &mut dyn SandboxScope, id: Uuid, approval: &TestApproval) -> Result<Judged> {
	let bundle = scope.approval_policy().await?;
	let session = scope.session(id, true).await?;
	let standing = standing(
		&bundle,
		scope.approver().as_ref(),
		&session,
		approval,
		scope.outbound_admitted(&approval.resource.id),
	);
	Ok(Judged { session, standing })
}

/// Whether the actor may test the session's draft. Only someone who may can
/// be offered, see or answer a request; losing it does not end a wait.
async fn may_test(scope: &mut dyn SandboxScope, draft: &Draft) -> Result<bool> {
	match scope.authorize_draft(draft, "agent_draft.test", true).await {
		Ok(()) => Ok(true),
		Err(Error::Forbidden) => Ok(false),
		Err(error) => Err(error),
	}
}

/// A refused identity or policy lock refuses the approval; nothing else does.
fn refused<T>(result: Result<T>) -> Result<Option<T>> {
	match result {
		Ok(value) => Ok(Some(value)),
		Err(Error::Forbidden | Error::Unauthorized) => Ok(None),
		Err(error) => Err(error),
	}
}

/// Ask the starter and wait for the answer. `true` lets the call go on to
/// dispatch, which rechecks it. No answer ever grants a Run.
pub async fn ask(
	repository: &dyn RealDispatchRepository,
	id: Uuid,
	approval: &TestApproval,
) -> Result<bool> {
	let request = Uuid::now_v7();
	{
		let mut scope = repository.begin_real().await?;
		let Some(judged) = refused(judge(scope.as_mut(), id, approval).await)? else {
			return Ok(false);
		};
		if judged.session.status != "running" {
			return Ok(false);
		}
		match judged.standing {
			Standing::Allowed => return Ok(true),
			Standing::Refused => return Ok(false),
			Standing::Askable => {}
		}
		let draft = scope.draft(judged.session.draft_id, false).await?;
		if !may_test(scope.as_mut(), &draft).await? {
			return Ok(false);
		}
		let mut scenario = judged.session.scenario;
		scenario["pending_approval"] = json!({"id":request,"call":approval.call,"resource":approval.resource,"action":approval.action,"state":"pending"});
		scope.write_scenario(id, scenario).await?;
		scope.commit().await?;
	}
	loop {
		tokio::time::sleep(POLL).await;
		let mut scope = repository.begin_real().await?;
		let Some(judged) = refused(judge(scope.as_mut(), id, approval).await)? else {
			return Ok(false);
		};
		let pending = &judged.session.scenario["pending_approval"];
		if judged.session.status != "running"
			|| pending["id"] != json!(request)
			|| pending["state"] == "deny"
		{
			return Ok(false);
		}
		// A change of policy settles the request before any answer does.
		let settled = match judged.standing {
			Standing::Askable => None,
			Standing::Allowed => Some("superseded"),
			Standing::Refused => Some("refused"),
		};
		if let Some(state) = settled {
			let mut scenario = judged.session.scenario;
			scenario["pending_approval"]["state"] = json!(state);
			scope.write_scenario(id, scenario).await?;
			scope.commit().await?;
			return Ok(judged.standing == Standing::Allowed);
		}
		match pending["state"].as_str() {
			Some("pending") => scope.commit().await?,
			Some("allow_once") => return Ok(true),
			_ => return Ok(false),
		}
	}
}

/// Recheck an approval in the dispatch transaction, and with `consume` use
/// up its one allowance. Dispatch checks `agent_draft.test` itself.
pub async fn recheck(
	scope: &mut dyn SandboxScope,
	id: Uuid,
	approval: &TestApproval,
	consume: bool,
) -> Result<()> {
	let judged = judge(&mut *scope, id, approval).await?;
	match judged.standing {
		Standing::Allowed => return Ok(()),
		Standing::Refused => return Err(Error::Forbidden),
		Standing::Askable => {}
	}
	let pending = &judged.session.scenario["pending_approval"];
	if judged.session.status != "running"
		|| pending["state"] != "allow_once"
		|| pending["resource"] != serde_json::to_value(&approval.resource)?
		|| pending["call"] != serde_json::to_value(&approval.call)?
		|| pending["action"] != approval.action
	{
		return Err(Error::Forbidden);
	}
	if consume {
		let mut scenario = judged.session.scenario;
		scenario["pending_approval"]["state"] = json!("consumed");
		scope.write_scenario(id, scenario).await?;
	}
	Ok(())
}

/// The starter answers the request `approval_id` of session `id`.
pub async fn answer(
	repository: &dyn SandboxRepository,
	id: Uuid,
	approval_id: Uuid,
	choice: Choice,
) -> Result<TestSession> {
	let mut scope = repository.begin().await?;
	let bundle = scope.approval_policy().await?;
	let session = scope.session(id, true).await?;
	let pending = &session.scenario["pending_approval"];
	if session.status != "running"
		|| pending["id"] != json!(approval_id)
		|| pending["state"] != "pending"
	{
		return Err(Error::Forbidden);
	}
	let approval = requested(pending).ok_or(Error::Forbidden)?;
	let askable = standing(
		&bundle,
		scope.approver().as_ref(),
		&session,
		&approval,
		scope.outbound_admitted(&approval.resource.id),
	) == Standing::Askable;
	let draft = scope.draft(session.draft_id, false).await?;
	if !askable || !may_test(scope.as_mut(), &draft).await? {
		return Err(Error::Forbidden);
	}
	let mut scenario = session.scenario;
	scenario["pending_approval"]["state"] = json!(match choice {
		Choice::AllowOnce => "allow_once",
		Choice::Deny => "deny",
	});
	let mut result = scope.write_scenario(id, scenario).await?;
	scope.commit().await?;
	conceal(&mut result);
	Ok(result)
}

/// Keep in `rows` only the requests this actor may answer now, and remove
/// who started each test.
pub(super) async fn reveal(
	scope: &mut dyn SandboxScope,
	draft: &Draft,
	rows: &mut [TestSession],
) -> Result<()> {
	let viewer = match scope.approver() {
		Some(approver) if approver.person.is_some() => {
			let bundle = scope.approval_policy().await?;
			may_test(scope, draft).await?.then_some((approver, bundle))
		}
		_ => None,
	};
	for row in rows {
		let visible = viewer.as_ref().is_some_and(|(approver, bundle)| {
			let pending = &row.scenario["pending_approval"];
			row.status == "running"
				&& pending["state"] == "pending"
				&& requested(pending).is_some_and(|approval| {
					standing(
						bundle,
						Some(approver),
						row,
						&approval,
						scope.outbound_admitted(&approval.resource.id),
					) == Standing::Askable
				})
		});
		if visible {
			strip_starter(row);
		} else {
			conceal(row);
		}
	}
	Ok(())
}

/// Remove who started a test and any request it holds.
pub fn conceal(session: &mut TestSession) {
	strip_starter(session);
	if let Some(scenario) = session.scenario.as_object_mut() {
		scenario.remove("pending_approval");
	}
}

fn strip_starter(session: &mut TestSession) {
	if let Some(scenario) = session.scenario.as_object_mut() {
		scenario.remove("starter_identity");
		scenario.remove("starter_subject");
	}
}

#[cfg(test)]
mod tests;

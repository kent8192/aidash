//! Durable pending evidence precedes network I/O, followed by fresh authority held through dispatch.
use crate::{
	Error, Result,
	ports::{
		Credentials,
		registry::workbench::{
			profile::ProfileConfiguration,
			sandbox::dispatch::{
				PreparedRealRequest, RealDispatchRepository, RealDispatchScope, RealToolTransport,
			},
		},
	},
};
use aidash_domain::{
	provider::ToolCall,
	registry::workbench::{
		profile::RealToolRule,
		sandbox::{ProfilePin, TestSession},
	},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;
pub struct Dispatch<'a> {
	pub repository: &'a dyn RealDispatchRepository,
	pub credentials: &'a dyn Credentials,
	pub configuration: &'a dyn ProfileConfiguration,
	pub transport: &'a dyn RealToolTransport,
}
pub fn credential_fingerprint(credentials: &dyn Credentials, name: &str) -> Result<Vec<u8>> {
	Ok(Sha256::digest(credentials.resolve(name)?.as_bytes()).to_vec())
}
async fn prepare(
	scope: &mut dyn RealDispatchScope,
	dispatch: &Dispatch<'_>,
	session_id: Uuid,
	pin: &ProfilePin,
	rule: &RealToolRule,
	call: &ToolCall,
) -> Result<(TestSession, Box<dyn PreparedRealRequest>)> {
	let session = scope.session(session_id, true).await?;
	if session.status != "running" {
		return Err(Error::Conflict("test was stopped".into()));
	}
	scope.lock_identity().await?;
	let draft = scope.draft(session.draft_id, true).await?;
	if draft.archived || draft.revision != session.revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	scope
		.authorize_draft(&draft, "agent_draft.test", true)
		.await?;
	let current = scope.profile(&pin.tenant, &pin.id).await?;
	if !current.enabled
		|| current.revision != pin.revision
		|| current.rules != serde_json::to_value(&pin.rules)?
	{
		return Err(Error::Conflict(
			"test profile changed or was disabled".into(),
		));
	}
	for (name, fingerprint) in &pin.credential_fingerprints {
		if credential_fingerprint(dispatch.credentials, name)? != *fingerprint {
			return Err(Error::Conflict("test credential changed".into()));
		}
	}
	let tool = scope.effective(&rule.tool).await?;
	super::super::profile::validate_real_rule(dispatch.configuration, rule, &tool)?;
	jsonschema::validator_for(&tool.schema)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.validate(&call.arguments)
		.map_err(|e| Error::Invalid(e.to_string()))?;
	let request = dispatch.transport.prepare(session_id, rule, call)?;
	Ok((session, request))
}
async fn record_pre_dispatch_denial(
	repository: &dyn RealDispatchRepository,
	session_id: Uuid,
	call: &ToolCall,
	error: &Error,
) -> Result<()> {
	let mut scope = repository.begin_real().await?;
	let session = scope.session(session_id, true).await?;
	let mut recorded = session.tool_calls.unwrap_or_else(|| json!([]));
	if let Some(pending) = recorded
		.as_array_mut()
		.and_then(|calls| calls.last_mut())
		.filter(|pending| pending["id"] == call.id && pending["outcome"] == "outcome_unknown")
	{
		pending["outcome"] = json!("denied");
		pending["error"] = json!(error.to_string());
		scope.write_calls(session_id, recorded, false).await?;
	}
	scope.commit().await
}
pub async fn invoke(
	dispatch: &Dispatch<'_>,
	session_id: Uuid,
	pin: &ProfilePin,
	rule: &RealToolRule,
	call: &ToolCall,
) -> Result<(Value, &'static str)> {
	let repository = dispatch.repository;
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
	let pending = json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"outcome_unknown","endpoint":rule.endpoint});
	let mut scope = repository.begin_real().await?;
	let (session, _) = prepare(scope.as_mut(), dispatch, session_id, pin, rule, call).await?;
	let mut calls = session.tool_calls.unwrap_or_else(|| json!([]));
	calls
		.as_array_mut()
		.ok_or_else(|| Error::Invalid("invalid test call log".into()))?
		.push(pending);
	scope.write_calls(session_id, calls, true).await?;
	scope.commit().await?;
	let mut scope = repository.begin_real().await?;
	let (_, request) = match prepare(scope.as_mut(), dispatch, session_id, pin, rule, call).await {
		Ok(prepared) => prepared,
		Err(error) => {
			scope.rollback().await?;
			record_pre_dispatch_denial(repository, session_id, call, &error).await?;
			return Err(error);
		}
	};
	let (result, outcome) = request.send().await?;
	let mut recorded = scope
		.session(session_id, false)
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
	scope.write_calls(session_id, recorded, false).await?;
	scope.commit().await?;
	Ok((result, outcome))
}
#[cfg(test)]
mod tests;

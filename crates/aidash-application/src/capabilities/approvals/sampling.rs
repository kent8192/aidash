//! Sampling approvals name a server, never one of the model's tool aliases.
use super::*;
use crate::ports::capabilities::approvals::SamplingApprovalRequest;
use aidash_domain::agent_plugin::interaction::SamplingTarget;

pub(super) fn resource(scope: &dyn ApprovalScope, target: &SamplingTarget, run: Uuid) -> Resource {
	scope.resource(
		"sampling",
		&target.deny_target(),
		json!({"run_id":run,"target":target,"action":SAMPLE_UNAPPROVED}),
	)
}

pub async fn request(
	scope: &mut dyn ApprovalScope,
	run: &Run,
	request: SamplingApprovalRequest,
) -> Result<ToolCallApproval> {
	if run.phase.is_terminal()
		|| run.control != RunControl::Active
		|| request.target.agent != run.agent_id
	{
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let limits = scope.limits()?;
	let digest = aidash_domain::registry::rules::digest(&json!([
		SAMPLING,
		run.id,
		request.key,
		request.target,
		request.request
	]));
	let cache_key = derived_key(json!([SAMPLING, digest]));
	if let Some(previous) = scope.cached(cache_key, &digest).await? {
		let record = scope
			.load(
				serde_json::from_value(previous["operation_id"].clone())?,
				SAMPLING,
			)
			.await?;
		if record.state == "consumed" && !still_approved(scope, run, &record).await? {
			return Ok(ToolCallApproval::Refused(Refusal::Revoked));
		}
		let target = resource(scope, &request.target, run.id);
		if let Advice::Refuse { by, .. } = advise(
			scope,
			run,
			ApprovalKind::Sampling,
			&request.target.allow_target(),
			&target,
		)
		.await?
		{
			return Ok(ToolCallApproval::Refused(match by {
				RefusedBy::DontAsk => Refusal::DontAsk,
				RefusedBy::Rule(_) => Refusal::Rule,
				_ => Refusal::Forbidden,
			}));
		}
		return recorded_tool_call(scope, run, record).await;
	}
	if request.expires_at <= Utc::now() {
		return Ok(ToolCallApproval::Refused(Refusal::Expired));
	}
	let destination = request.target.allow_target();
	let target = resource(scope, &request.target, run.id);
	let direct = match ordinary(scope, &target, SAMPLE_UNAPPROVED) {
		Err(Error::Forbidden) => return Ok(ToolCallApproval::Refused(Refusal::Forbidden)),
		other => other?,
	};
	let grant = scope.grants(run.id).await?.into_iter().find(|grant| {
		grants(grant, SAMPLE_UNAPPROVED)
			&& grant.data["targets"] == json!([destination])
			&& grant.data["subjects"] == json!(scope.subjects())
			&& eligible(
				scope,
				grant.data["approver"].as_str().unwrap_or_default(),
				&grant.owner,
				&target,
			)
	});
	let designated = grant
		.as_ref()
		.and_then(|grant| grant.data["approver"].as_str().map(str::to_owned))
		.or_else(|| approver(scope, &target));
	let advice = if direct || grant.is_some() {
		Advice::Ask
	} else {
		advise(scope, run, ApprovalKind::Sampling, &destination, &target).await?
	};
	let state = advice
		.state("approved")
		.unwrap_or(approvals::requested_state(
			direct,
			grant.is_some(),
			designated.is_some(),
		));
	let mut data = json!({"run_id":run.id,"agent_id":run.agent_id,"agent_version":run.agent_version,
		"requester":scope.principal(),"credential_id":scope.credential(),"subjects":scope.subjects(),
		"targets":[destination],"action":SAMPLE_UNAPPROVED,"operation":"sampling/createMessage",
		"sampling_target":request.target,"sampling":request.request,"include_context":"none",
		"digest":digest,"invocation_key":request.key,"policy_revision":scope.policy_revision(),
		"approver":designated,"grant_id":grant.map(|grant| grant.id),"ordinary":direct});
	advice.record(&mut data);
	let id = Uuid::new_v4();
	let record = scope
		.insert(Creation {
			id,
			area: request.area,
			kind: SAMPLING,
			state: if state == "approved" {
				"consumed"
			} else {
				state
			},
			data,
			expires: Some(
				request
					.expires_at
					.min(Utc::now() + Duration::seconds(limits.approval_seconds as i64)),
			),
		})
		.await?;
	scope
		.cache(cache_key, &digest, &json!({"operation_id":id}))
		.await?;
	scope.event(run.workspace_id,"capability.approval_requested",json!({"id":id,"run_id":run.id,"kind":SAMPLING,"status":state,"targets":record.data["targets"],"approver":record.data["approver"]})).await?;
	Ok(settled_tool_call(&record))
}

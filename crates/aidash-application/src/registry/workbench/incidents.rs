//! Manually reported incidents use current authority and atomic source-attributed history.
use crate::{
	Error, Result,
	ports::registry::workbench::incidents::{
		IncidentRepository, IncidentRetentionRepository, IncidentScope,
	},
};
use aidash_domain::{
	identity::Principal,
	policy::{Evaluation, Resource},
	registry::{
		EntityRef,
		workbench::incident::{
			self, CreateIncident, EvidenceCopy, EvidenceInput, Incident, IncidentEvent,
			UpdateIncident,
		},
	},
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
fn actor_name(principal: Principal) -> String {
	match principal {
		Principal::Operator => "operator".into(),
		Principal::Subject { subject, .. } => subject,
	}
}
fn fixed_copies(evidence: Vec<EvidenceInput>) -> Vec<EvidenceCopy> {
	evidence
		.into_iter()
		.map(|item| incident::fixed_copy(item, Utc::now()))
		.collect()
}
pub async fn require(
	scope: &mut dyn IncidentScope,
	incident: &Incident,
	action: &str,
) -> Result<()> {
	let Principal::Subject { tenant, subject } = scope.principal() else {
		return Ok(());
	};
	if tenant != incident.tenant {
		return Err(Error::Forbidden);
	}
	scope.lock_identity().await?;
	let decision=scope.evaluate(&incident.tenant,&Evaluation {
  subject,action:action.into(),resource:Resource {tenant:incident.tenant.clone(),kind:"agent_incident".into(),id:incident.id.to_string(),attributes:json!({"agent_id":incident.agent_id,"version":incident.version,"owner":incident.owner,"status":incident.status,"archived":incident.archived})},environment:json!({}),
 }).await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
pub async fn create(
	repository: &dyn IncidentRepository,
	reference: EntityRef,
	input: CreateIncident,
) -> Result<Incident> {
	incident::validate(&input.severity, "open", &input.notes, &input.evidence)?;
	let actor = actor_name(repository.principal());
	let (tenant, _) = super::author_identity(
		&repository.principal(),
		input.tenant.as_deref(),
		Some(&actor),
	)?;
	let mut scope = repository.begin().await?;
	scope.target_enabled(&tenant, &input.owner).await?;
	scope.require_inspection(&reference).await?;
	if scope.effective(&reference).await?.kind != "agent" {
		return Err(Error::NotFound("agent version".into()));
	}
	let candidate = Incident {
		id: Uuid::now_v7(),
		tenant,
		agent_id: reference.id,
		version: reference.version,
		revision: 1,
		severity: input.severity,
		status: "open".into(),
		archived: false,
		owner: input.owner,
		notes: input.notes,
		evidence: serde_json::to_value(fixed_copies(input.evidence))?,
		created_at: Utc::now(),
		updated_at: Utc::now(),
		resolved_at: None,
		evidence_expires_at: None,
		evidence_expired_at: None,
	};
	require(scope.as_mut(), &candidate, "agent_incident.manage").await?;
	scope.insert(&candidate).await?;
	scope.record_event(candidate.id,&actor,json!({"created":true,"severity":candidate.severity,"status":"open","evidence_count":candidate.evidence.as_array().map_or(0,Vec::len)})).await?;
	let result = scope.read(candidate.id, false).await?;
	scope.commit().await?;
	Ok(result)
}
pub async fn list(
	repository: &dyn IncidentRepository,
	reference: EntityRef,
) -> Result<Vec<Incident>> {
	let mut scope = repository.begin().await?;
	scope.require_inspection(&reference).await?;
	let tenant = match repository.principal() {
		Principal::Operator => None,
		Principal::Subject { tenant, .. } => Some(tenant),
	};
	let mut visible = Vec::new();
	let mut cursor = None;
	loop {
		let rows = scope.page(&reference, tenant.as_deref(), cursor).await?;
		let more = rows.len() == 100;
		cursor = rows.last().map(|r| (r.created_at, r.id));
		for row in rows {
			match require(scope.as_mut(), &row, "agent_incident.read").await {
				Ok(()) => visible.push(row),
				Err(Error::Forbidden) => {}
				Err(error) => return Err(error),
			}
			if visible.len() == 100 {
				break;
			}
		}
		if visible.len() == 100 || !more {
			break;
		}
	}
	scope.commit().await?;
	Ok(visible)
}
pub async fn get(repository: &dyn IncidentRepository, id: Uuid) -> Result<Incident> {
	let mut scope = repository.begin().await?;
	let incident = scope.read(id, false).await?;
	scope
		.require_inspection(&EntityRef {
			id: incident.agent_id.clone(),
			version: incident.version.clone(),
		})
		.await?;
	require(scope.as_mut(), &incident, "agent_incident.read").await?;
	scope.commit().await?;
	Ok(incident)
}
pub async fn read_events(
	repository: &dyn IncidentRepository,
	id: Uuid,
	limit: usize,
) -> Result<Vec<IncidentEvent>> {
	let mut scope = repository.begin().await?;
	let incident = scope.read(id, true).await?;
	scope
		.require_inspection(&EntityRef {
			id: incident.agent_id.clone(),
			version: incident.version.clone(),
		})
		.await?;
	require(scope.as_mut(), &incident, "agent_incident.read").await?;
	let events = scope.events(id, limit).await?;
	scope.commit().await?;
	Ok(events)
}
pub async fn events(repository: &dyn IncidentRepository, id: Uuid) -> Result<Vec<IncidentEvent>> {
	let mut events = read_events(repository, id, 500).await?;
	events.reverse();
	Ok(events)
}
pub async fn update(
	repository: &dyn IncidentRepository,
	id: Uuid,
	input: UpdateIncident,
) -> Result<Incident> {
	incident::validate(
		&input.severity,
		&input.status,
		&input.notes,
		&input.add_evidence,
	)?;
	let mut scope = repository.begin().await?;
	let prior = scope.read(id, true).await?;
	scope
		.require_inspection(&EntityRef {
			id: prior.agent_id.clone(),
			version: prior.version.clone(),
		})
		.await?;
	require(scope.as_mut(), &prior, "agent_incident.manage").await?;
	if prior.revision != input.expected_revision {
		return Err(Error::Conflict("incident revision changed".into()));
	}
	scope.target_enabled(&prior.tenant, &input.owner).await?;
	let mut evidence: Vec<EvidenceCopy> = serde_json::from_value(prior.evidence.clone())?;
	if prior.evidence_expired_at.is_some() && !input.add_evidence.is_empty() {
		return Err(Error::Conflict(
			"expired evidence cannot be revived; open a new incident".into(),
		));
	}
	evidence.extend(fixed_copies(input.add_evidence));
	if evidence.len() > 32 {
		return Err(Error::Invalid("incident evidence exceeds 32 copies".into()));
	}
	let (resolved_at, expires_at) = match (prior.status == "resolved", input.status == "resolved") {
		(false, true) => {
			let days = scope.evidence_days(&prior.tenant).await?;
			let now = Utc::now();
			(
				Some(now),
				Some(now + chrono::Duration::days(i64::from(days))),
			)
		}
		(true, true) => (prior.resolved_at, prior.evidence_expires_at),
		_ => (None, None),
	};
	let updated = Incident {
		severity: input.severity.clone(),
		status: input.status.clone(),
		owner: input.owner.clone(),
		notes: input.notes,
		evidence: serde_json::to_value(&evidence)?,
		resolved_at,
		evidence_expires_at: expires_at,
		archived: input.archived.unwrap_or(prior.archived),
		..prior.clone()
	};
	scope.save(&updated).await?;
	scope.record_event(id,&actor_name(repository.principal()),json!({"from_revision":prior.revision,"severity":input.severity,"status":input.status,"archived":input.archived.unwrap_or(prior.archived),"owner":input.owner,"evidence_count":evidence.len()})).await?;
	let result = scope.read(id, false).await?;
	scope.commit().await?;
	Ok(result)
}
pub async fn purge_expired(repository: &dyn IncidentRetentionRepository) -> Result<u64> {
	let mut count = 0;
	loop {
		let mut scope = repository.begin().await?;
		let rows = scope.expired().await?;
		let expired = rows.len();
		for row in rows {
			let mut copies: Vec<EvidenceCopy> = serde_json::from_value(row.evidence)?;
			for copy in &mut copies {
				copy.content = None;
			}
			scope.discard(row.id, serde_json::to_value(copies)?).await?;
		}
		scope.commit().await?;
		count += expired as u64;
		if expired == 0 {
			return Ok(count);
		}
	}
}
#[cfg(test)]
mod tests;

//! Manually reported version incidents. Severity is the reporter's label, not
//! an Aidash assessment. Evidence is a fixed copy with a separate retention clock.
use super::*;
use crate::apps::registry::workbench::models::{AgentIncident, AgentIncidentEvent, AgentTestLimit};
use crate::registry::EntityRef;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, PostgresBackend};
use reinhardt::db::orm::DatabaseConnectionLease;
use reinhardt::injectable;
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn validate(severity: &str, status: &str, notes: &str, evidence: &[EvidenceInput]) -> Result<()> {
	if !matches!(severity, "low" | "medium" | "high" | "critical")
		|| !matches!(status, "open" | "resolved")
		|| notes.trim().is_empty()
		|| notes.len() > 16_384
		|| evidence.len() > 8
		|| evidence
			.iter()
			.map(|item| item.content.len())
			.sum::<usize>()
			> 65_536
		|| evidence.iter().any(|item| {
			item.title.trim().is_empty() || item.title.len() > 255 || item.content.trim().is_empty()
		}) {
		return Err(Error::Invalid(
			"invalid incident severity, status, notes or evidence".into(),
		));
	}
	Ok(())
}

fn fixed_copies(evidence: Vec<EvidenceInput>) -> Vec<EvidenceCopy> {
	evidence
		.into_iter()
		.map(|item| EvidenceCopy {
			sha256: format!("{:x}", Sha256::digest(item.content.as_bytes())),
			title: item.title,
			content: Some(item.content),
			recorded_at: Utc::now(),
		})
		.collect()
}

fn actor_name(actor: &Actor) -> &str {
	match actor {
		Actor::Operator => "operator",
		Actor::Subject(identity) => &identity.subject,
	}
}

async fn require_incident(
	tx: &mut dyn TransactionExecutor,
	actor: &Actor,
	incident: &Incident,
	action: &str,
) -> Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	if identity.tenant != incident.tenant {
		return Err(Error::Forbidden);
	}
	identity.lock_native(tx, false).await?;
	let decision = Authorization::evaluate_native(tx, &incident.tenant, &Evaluation {
		subject: identity.subject.clone(), action: action.into(),
		resource: Resource { tenant: incident.tenant.clone(), kind: "agent_incident".into(), id: incident.id.to_string(), attributes: json!({"agent_id":incident.agent_id,"version":incident.version,"owner":incident.owner,"status":incident.status,"archived":incident.archived}) },
		environment: json!({}),
	}).await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

/// Read event history while holding current incident and authorization leases.
pub(crate) async fn read_events(
	f: &Federation,
	actor: &Actor,
	id: Uuid,
	limit: u64,
) -> Result<Vec<IncidentEvent>> {
	let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
	let incident = AgentIncident::read(&mut tx, id, true).await?;
	trust::require_inspection(
		&mut tx,
		actor,
		&EntityRef {
			id: incident.agent_id.clone(),
			version: incident.version.clone(),
		},
	)
	.await?;
	require_incident(&mut tx, actor, &incident, "agent_incident.read").await?;
	let events = AgentIncidentEvent::page(&mut tx, id, limit as usize).await?;
	Box::new(tx).commit().await?;
	Ok(events)
}

/// Expire copied payloads while keeping source, digest, actor and status data.
pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	let lease = DatabaseConnectionLease::register(BackendConnection::new(Arc::new(
		PostgresBackend::new(pool.clone()),
	)))?;
	let mut count = 0;
	loop {
		let expired: usize = lease
			.handle()
			.atomic(async |tx| {
				let rows = AgentIncident::expired(tx).await?;
				let count = rows.len();
				for row in rows {
					let mut copies: Vec<EvidenceCopy> = serde_json::from_value(row.evidence)?;
					for copy in &mut copies {
						copy.content = None;
					}
					AgentIncident::discard_evidence(tx, row.id, serde_json::to_value(copies)?)
						.await?;
				}
				Ok::<_, Error>(count)
			})
			.await?;
		count += expired as u64;
		if expired == 0 {
			return Ok(count);
		}
	}
}

pub use crate::apps::registry::workbench::serializers::incident::{
	CreateIncident, EvidenceCopy, EvidenceInput, Incident, IncidentEvent, UpdateIncident,
};

#[derive(Clone)]
pub struct Incidents {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_incident(#[inject] runtime: Federation) -> Incidents {
	Incidents { runtime }
}

impl Incidents {
	pub(crate) async fn create(
		&self,
		actor: Actor,
		(id, version): (String, String),
		input: CreateIncident,
	) -> Result<Incident> {
		let f = self.runtime.clone();
		validate(&input.severity, "open", &input.notes, &input.evidence)?;
		let (tenant, _) =
			author_identity(&actor, input.tenant.as_deref(), Some(actor_name(&actor)))?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		target_enabled(&mut tx, &tenant, &input.owner).await?;
		let reference = EntityRef { id, version };
		trust::require_inspection(&mut tx, &actor, &reference).await?;
		if admission::effective(&mut tx, &reference.id, &reference.version)
			.await?
			.kind != "agent"
		{
			return Err(Error::NotFound("agent version".into()));
		}
		let incident_id = Uuid::now_v7();
		let candidate = Incident {
			id: incident_id,
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
		require_incident(&mut tx, &actor, &candidate, "agent_incident.manage").await?;
		AgentIncident::insert(&mut tx, &candidate).await?;
		AgentIncidentEvent::record(&mut tx, incident_id, actor_name(&actor), json!({"created":true,"severity":candidate.severity,"status":"open","evidence_count":candidate.evidence.as_array().map_or(0,Vec::len)})).await?;
		let result = AgentIncident::read(&mut tx, incident_id, false).await?;
		Box::new(tx).commit().await?;
		Ok(result)
	}
	pub(crate) async fn list(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Vec<Incident>> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		trust::require_inspection(
			&mut tx,
			&actor,
			&EntityRef {
				id: id.clone(),
				version: version.clone(),
			},
		)
		.await?;
		let mut visible = Vec::new();
		let mut cursor: Option<(DateTime<Utc>, Uuid)> = None;
		let tenant = match &actor {
			Actor::Operator => None,
			Actor::Subject(identity) => Some(&identity.tenant),
		};
		loop {
			let rows =
				AgentIncident::page(&mut tx, &id, &version, tenant.map(String::as_str), cursor)
					.await?;
			let more = rows.len() == 100;
			cursor = rows.last().map(|row| (row.created_at, row.id));
			for row in rows {
				match require_incident(&mut tx, &actor, &row, "agent_incident.read").await {
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
		Box::new(tx).commit().await?;
		Ok(visible)
	}
	pub(crate) async fn get(&self, actor: Actor, id: Uuid) -> Result<Incident> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let incident = AgentIncident::read(&mut tx, id, false).await?;
		trust::require_inspection(
			&mut tx,
			&actor,
			&EntityRef {
				id: incident.agent_id.clone(),
				version: incident.version.clone(),
			},
		)
		.await?;
		require_incident(&mut tx, &actor, &incident, "agent_incident.read").await?;
		Box::new(tx).commit().await?;
		Ok(incident)
	}
	pub(crate) async fn update(
		&self,
		actor: Actor,
		id: Uuid,
		input: UpdateIncident,
	) -> Result<Incident> {
		let f = self.runtime.clone();
		validate(
			&input.severity,
			&input.status,
			&input.notes,
			&input.add_evidence,
		)?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let prior = AgentIncident::read(&mut tx, id, true).await?;
		trust::require_inspection(
			&mut tx,
			&actor,
			&EntityRef {
				id: prior.agent_id.clone(),
				version: prior.version.clone(),
			},
		)
		.await?;
		require_incident(&mut tx, &actor, &prior, "agent_incident.manage").await?;
		if prior.revision != input.expected_revision {
			return Err(Error::Conflict("incident revision changed".into()));
		}
		target_enabled(&mut tx, &prior.tenant, &input.owner).await?;
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
		let was_resolved = prior.status == "resolved";
		let resolved = input.status == "resolved";
		let (resolved_at, expires_at) = match (was_resolved, resolved) {
			(false, true) => {
				let days = AgentTestLimit::locked(&mut tx, &prior.tenant)
					.await?
					.incident_evidence_days;
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
		AgentIncident::save(&mut tx, &updated).await?;
		AgentIncidentEvent::record(&mut tx, id, actor_name(&actor), json!({"from_revision":prior.revision,"severity":input.severity,"status":input.status,"archived":input.archived.unwrap_or(prior.archived),"owner":input.owner,"evidence_count":evidence.len()})).await?;
		let result = AgentIncident::read(&mut tx, id, false).await?;
		Box::new(tx).commit().await?;
		Ok(result)
	}
	pub(crate) async fn events(&self, actor: Actor, id: Uuid) -> Result<Vec<IncidentEvent>> {
		let f = self.runtime.clone();
		let mut events = read_events(&f, &actor, id, 500).await?;
		events.reverse();
		Ok(events)
	}
}

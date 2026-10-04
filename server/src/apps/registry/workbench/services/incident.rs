//! Manually reported version incidents. Severity is the reporter's label, not
//! an Aidash assessment. Evidence is a fixed copy with a separate retention clock.
use super::*;
use crate::registry::EntityRef;
use reinhardt::injectable;

pub(crate) async fn read_events(
	f: &Federation,
	actor: &Actor,
	id: Uuid,
	limit: u64,
) -> Result<Vec<IncidentEvent>> {
	aidash_application::registry::workbench::incidents::read_events(
		&crate::bootstrap::workbench_incident_repository(f, actor.clone()),
		id,
		limit as usize,
	)
	.await
	.map_err(Into::into)
}

pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	aidash_application::registry::workbench::incidents::purge_expired(
		&crate::bootstrap::workbench_incident_retention_repository(pool),
	)
	.await
	.map_err(Into::into)
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
		aidash_application::registry::workbench::incidents::create(
			&crate::bootstrap::workbench_incident_repository(&self.runtime, actor),
			EntityRef { id, version },
			input,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn list(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Vec<Incident>> {
		aidash_application::registry::workbench::incidents::list(
			&crate::bootstrap::workbench_incident_repository(&self.runtime, actor),
			EntityRef { id, version },
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn get(&self, actor: Actor, id: Uuid) -> Result<Incident> {
		aidash_application::registry::workbench::incidents::get(
			&crate::bootstrap::workbench_incident_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn update(
		&self,
		actor: Actor,
		id: Uuid,
		input: UpdateIncident,
	) -> Result<Incident> {
		aidash_application::registry::workbench::incidents::update(
			&crate::bootstrap::workbench_incident_repository(&self.runtime, actor),
			id,
			input,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn events(&self, actor: Actor, id: Uuid) -> Result<Vec<IncidentEvent>> {
		aidash_application::registry::workbench::incidents::events(
			&crate::bootstrap::workbench_incident_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
}

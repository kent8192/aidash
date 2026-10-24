use serde::{Deserialize, Serialize};
// Serializable authority contracts.

#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
	pub(crate) credential_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) subject: String,
}

/// No state, artifact content, registry document or provider argument is sent
/// during preflight. Resource identifiers and the recipient set are explicit.
#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
	pub(crate) kind: String,
	pub(crate) id: Uuid,
	pub(crate) task_id: Option<Uuid>,
}

#[derive(
	Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preflight {
	pub(crate) id: Uuid,
	pub(crate) coordinator: String,
	pub(crate) digest: String,
	pub(crate) origin: Origin,
	pub(crate) recipients: Vec<String>,
	pub(crate) targets: Vec<Target>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub(crate) struct Binding {
	pub(crate) request: Preflight,
	pub(crate) local: Origin,
	pub(crate) subjects: Vec<String>,
}

use uuid::Uuid;

impl From<&Origin> for aidash_domain::transactions::authority::Origin {
	fn from(origin: &Origin) -> Self {
		Self {
			credential_id: origin.credential_id,
			tenant: origin.tenant.clone(),
			subject: origin.subject.clone(),
		}
	}
}
impl From<aidash_domain::transactions::authority::Preflight> for Preflight {
	fn from(input: aidash_domain::transactions::authority::Preflight) -> Self {
		Self {
			id: input.id,
			coordinator: input.coordinator,
			digest: input.digest,
			origin: Origin {
				credential_id: input.origin.credential_id,
				tenant: input.origin.tenant,
				subject: input.origin.subject,
			},
			recipients: input.recipients,
			targets: input
				.targets
				.into_iter()
				.map(|target| Target {
					kind: target.kind,
					id: target.id,
					task_id: target.task_id,
				})
				.collect(),
		}
	}
}
impl From<&Preflight> for aidash_domain::transactions::authority::Preflight {
	fn from(input: &Preflight) -> Self {
		Self {
			id: input.id,
			coordinator: input.coordinator.clone(),
			digest: input.digest.clone(),
			origin: (&input.origin).into(),
			recipients: input.recipients.clone(),
			targets: input
				.targets
				.iter()
				.map(|target| aidash_domain::transactions::authority::Target {
					kind: target.kind.clone(),
					id: target.id,
					task_id: target.task_id,
				})
				.collect(),
		}
	}
}

impl From<aidash_domain::transactions::authority::Origin> for Origin {
	fn from(origin: aidash_domain::transactions::authority::Origin) -> Self {
		Self {
			credential_id: origin.credential_id,
			tenant: origin.tenant,
			subject: origin.subject,
		}
	}
}
impl From<&Binding> for aidash_domain::transactions::authority::Binding {
	fn from(binding: &Binding) -> Self {
		Self {
			request: (&binding.request).into(),
			local: (&binding.local).into(),
			subjects: binding.subjects.clone(),
		}
	}
}
impl From<&aidash_domain::transactions::authority::Binding> for Binding {
	fn from(binding: &aidash_domain::transactions::authority::Binding) -> Self {
		Self {
			request: binding.request.clone().into(),
			local: binding.local.clone().into(),
			subjects: binding.subjects.clone(),
		}
	}
}

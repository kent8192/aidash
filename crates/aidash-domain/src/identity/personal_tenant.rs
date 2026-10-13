//! A private Tenant belongs to one verified External Identity, never an email address.
use crate::policy::{
	Effect, Policy, PolicyBundle, ResourceSelector, Subject, SubjectKind, SubjectSelector,
};
use uuid::Uuid;

pub const OWNER: &str = "owner";

pub struct PersonalTenant {
	pub tenant: String,
}

impl PersonalTenant {
	pub fn for_identity(identity: Uuid) -> Self {
		Self {
			tenant: format!("personal-{}", identity.simple()),
		}
	}
	pub fn contains(&self, tenant: &str, subject: &str) -> bool {
		tenant == self.tenant && subject == OWNER
	}
	/// Ordinary Subject authority is limited by the bundle's Tenant boundary.
	/// Operator authority is a separate, explicitly granted Principal.
	pub fn policy(&self) -> PolicyBundle {
		PolicyBundle {
			tenant: self.tenant.clone(),
			subjects: [(
				OWNER.into(),
				Subject {
					kind: SubjectKind::User,
					groups: Default::default(),
					roles: Default::default(),
					attributes: serde_json::json!({}),
					enabled: true,
					delegated_by: None,
				},
			)]
			.into(),
			groups: Default::default(),
			roles: Default::default(),
			policies: vec![Policy {
				id: "personal-owner".into(),
				effect: Effect::Allow,
				subjects: SubjectSelector {
					ids: [OWNER.into()].into(),
					..Default::default()
				},
				actions: ["*".into()].into(),
				resources: ResourceSelector {
					kinds: ["*".into()].into(),
					ids: Default::default(),
				},
				condition: None,
			}],
		}
	}
}

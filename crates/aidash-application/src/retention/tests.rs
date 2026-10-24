use super::*;
use crate::ports::retention::RetentionScope;
use aidash_domain::retention::SnapshotPeriods;
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Mutex};

/// Stored Tenant periods with their history of deciders.
struct Tenants {
	decider: Decider,
	permitted: bool,
	stored: Mutex<BTreeMap<String, RetentionRules>>,
	history: Mutex<Vec<(String, Option<RetentionRules>, String)>>,
}

impl Tenants {
	fn new(decider: Decider, permitted: bool) -> Self {
		Self {
			decider,
			permitted,
			stored: Mutex::default(),
			history: Mutex::default(),
		}
	}
}

/// Writes reach the store only when the scope commits.
struct Scope<'a> {
	tenants: &'a Tenants,
	pending: Vec<(String, Option<RetentionRules>, String)>,
}

#[async_trait]
impl RetentionScope for Scope<'_> {
	fn decider(&self) -> Decider {
		self.tenants.decider.clone()
	}
	fn deployment(&self) -> RetentionRules {
		RetentionRules::DEFAULT
	}
	async fn permits(&mut self, _: &str, action: &str) -> Result<bool> {
		assert_eq!(action, "retention.update");
		Ok(matches!(
			(&self.tenants.decider, self.tenants.permitted),
			(Decider::DeploymentAdministrator { .. }, _) | (_, true)
		))
	}
	async fn stored(&mut self, tenant: &str) -> Result<Option<RetentionRules>> {
		Ok(self.tenants.stored.lock().unwrap().get(tenant).copied())
	}
	async fn store(
		&mut self,
		tenant: &str,
		rules: Option<&RetentionRules>,
		decider: &Decider,
	) -> Result<()> {
		self.pending
			.push((tenant.into(), rules.copied(), decider.recorded()));
		Ok(())
	}
	async fn history(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<RetentionChange>> {
		Ok(self
			.tenants
			.history
			.lock()
			.unwrap()
			.iter()
			.rev()
			.filter(|(owner, ..)| owner == tenant)
			.skip(offset as usize)
			.take(limit as usize)
			.map(|(_, rules, decider)| RetentionChange {
				rules: *rules,
				changed_at: chrono::DateTime::UNIX_EPOCH,
				changed_by: decider.clone(),
			})
			.collect())
	}
	async fn finish(self: Box<Self>, committed: bool) -> Result<()> {
		if committed {
			for (tenant, rules, decider) in self.pending {
				let mut stored = self.tenants.stored.lock().unwrap();
				match rules {
					Some(rules) => stored.insert(tenant.clone(), rules),
					None => stored.remove(&tenant),
				};
				self.tenants
					.history
					.lock()
					.unwrap()
					.push((tenant, rules, decider));
			}
		}
		Ok(())
	}
}

#[async_trait]
impl RetentionRepository for Tenants {
	async fn begin(&self) -> Result<Box<dyn RetentionScope + '_>> {
		Ok(Box::new(Scope {
			tenants: self,
			pending: vec![],
		}))
	}
}

fn subject(tenant: &str) -> Decider {
	Decider::Subject {
		tenant: tenant.into(),
		subject: "user:ada".into(),
	}
}

fn shorter() -> RetentionRules {
	RetentionRules {
		document_snapshots: SnapshotPeriods {
			after_run_seconds: 3_600,
			max_age_seconds: 86_400,
		},
	}
}

#[tokio::test]
async fn a_permitted_subject_shortens_its_tenants_periods_with_history() {
	let tenants = Tenants::new(subject("acme"), true);
	let changed = update(&tenants, "acme", Some(shorter())).await.unwrap();
	assert_eq!(changed.tenant_rules, Some(shorter()));
	assert_eq!(changed.effective, shorter());
	assert_eq!(changed.deployment, RetentionRules::DEFAULT);
	assert_eq!(rules(&tenants, "acme").await.unwrap(), changed);
	assert_eq!(
		tenants.history.lock().unwrap().as_slice(),
		[(
			"acme".to_owned(),
			Some(shorter()),
			"subject:acme/user:ada".to_owned()
		)]
	);
}

#[tokio::test]
async fn clearing_the_periods_applies_the_deployments_again() {
	let tenants = Tenants::new(Decider::DeploymentAdministrator { identity: None }, false);
	update(&tenants, "acme", Some(shorter())).await.unwrap();
	let cleared = update(&tenants, "acme", None).await.unwrap();
	assert_eq!(cleared.tenant_rules, None);
	assert_eq!(cleared.effective, RetentionRules::DEFAULT);
	assert_eq!(tenants.history.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_tenant_cannot_keep_contents_longer_than_the_deployment() {
	let tenants = Tenants::new(subject("acme"), true);
	let mut longer = RetentionRules::DEFAULT;
	longer.document_snapshots.max_age_seconds += 1;
	assert!(matches!(
		update(&tenants, "acme", Some(longer)).await,
		Err(Error::Domain(_))
	));
	assert!(tenants.history.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_subject_changes_only_its_own_tenant_and_only_when_permitted() {
	let tenants = Tenants::new(subject("acme"), true);
	assert!(matches!(
		update(&tenants, "other", Some(shorter())).await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		rules(&tenants, "other").await,
		Err(Error::Forbidden)
	));
	let unpermitted = Tenants::new(subject("acme"), false);
	assert!(matches!(
		update(&unpermitted, "acme", Some(shorter())).await,
		Err(Error::Forbidden)
	));
	// Reading needs no permission beyond being a subject of the Tenant.
	assert_eq!(
		rules(&unpermitted, "acme").await.unwrap().effective,
		RetentionRules::DEFAULT
	);
}

#[tokio::test]
async fn history_is_newest_first_and_only_for_those_who_may_change_the_periods() {
	let tenants = Tenants::new(subject("acme"), true);
	let shorter = RetentionRules {
		document_snapshots: SnapshotPeriods {
			after_run_seconds: 3_600,
			max_age_seconds: 86_400,
		},
	};
	update(&tenants, "acme", Some(shorter)).await.unwrap();
	update(&tenants, "acme", None).await.unwrap();
	let changes = history(&tenants, "acme", 0, 10).await.unwrap();
	assert_eq!(
		changes
			.iter()
			.map(|change| change.rules)
			.collect::<Vec<_>>(),
		[None, Some(shorter)]
	);
	assert_eq!(changes[0].changed_by, "subject:acme/user:ada");
	assert_eq!(history(&tenants, "acme", 1, 10).await.unwrap().len(), 1);

	let reader = Tenants::new(subject("acme"), false);
	assert!(matches!(
		history(&reader, "acme", 0, 10).await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		history(&tenants, "other", 0, 10).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_dashboard_learns_who_may_change_the_periods() {
	assert!(
		may_update(&Tenants::new(subject("acme"), true), "acme")
			.await
			.unwrap()
	);
	assert!(
		!may_update(&Tenants::new(subject("acme"), false), "acme")
			.await
			.unwrap()
	);
	assert!(
		may_update(
			&Tenants::new(Decider::DeploymentAdministrator { identity: None }, false),
			"acme"
		)
		.await
		.unwrap()
	);
	// Another Tenant's periods stay as closed as they are to `rules`.
	assert!(matches!(
		may_update(&Tenants::new(subject("acme"), true), "other").await,
		Err(Error::Forbidden)
	));
}

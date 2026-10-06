use super::*;
use aidash_domain::deployment::DeploymentInventory;
use async_trait::async_trait;
use rstest::rstest;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Observer(AtomicUsize);
#[async_trait]
impl DeploymentObserver for Observer {
	async fn observe(&self) -> Result<DeploymentInventory> {
		self.0.fetch_add(1, Ordering::SeqCst);
		Ok(DeploymentInventory::default())
	}
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn subject_authority_is_rejected_before_any_external_observation(#[case] enabled: bool) {
	// Arrange
	let observer = Arc::new(Observer(AtomicUsize::new(0)));
	let service = DeploymentObservations::new(
		"tenant-a".into(),
		"release-a".into(),
		enabled.then(|| observer.clone() as Arc<dyn DeploymentObserver>),
	);
	let principal = Principal::Subject {
		tenant: "tenant-a".into(),
		subject: "alice".into(),
	};
	// Act
	let result = service.status(&principal).await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(observer.0.load(Ordering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn operator_observation_preserves_enabled_and_disabled_responses() {
	// Arrange
	let observer = Arc::new(Observer(AtomicUsize::new(0)));
	let service = DeploymentObservations::new(
		"tenant-a".into(),
		"release-a".into(),
		Some(observer.clone()),
	);
	let disabled = DeploymentObservations::new("".into(), "".into(), None);
	// Act
	let active = service.status(&Principal::Operator).await.unwrap();
	let inactive = disabled.status(&Principal::Operator).await.unwrap();
	// Assert
	assert!(active.enabled);
	assert_eq!(active.namespace.as_deref(), Some("tenant-a"));
	assert_eq!(active.release.as_deref(), Some("release-a"));
	assert_eq!(observer.0.load(Ordering::SeqCst), 1);
	assert!(!inactive.enabled);
	assert!(inactive.namespace.is_none());
	assert!(inactive.release.is_none());
}

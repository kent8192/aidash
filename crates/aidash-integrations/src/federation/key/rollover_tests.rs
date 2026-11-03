use super::*;
use aidash_application::federation::key::{
	DeploymentKeyRollover, DeploymentKeySource, DeploymentKeyStore, DeploymentKeys,
	ScheduledDeploymentKeys,
};
use aidash_domain::nodes::DeploymentId;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Schedule(Mutex<Option<DeploymentKeyRollover>>);
#[async_trait]
impl DeploymentKeyStore for Schedule {
	async fn load(&self) -> Result<Option<DeploymentKeyRollover>> {
		Ok(self.0.lock().unwrap().clone())
	}
	async fn compare_and_set(
		&self,
		expected: Option<&DeploymentKeyRollover>,
		next: &DeploymentKeyRollover,
	) -> Result<bool> {
		let mut state = self.0.lock().unwrap();
		if state.as_ref() != expected {
			return Ok(false);
		}
		*state = Some(next.clone());
		Ok(true)
	}
}

fn ring(initial: &Path, store: Arc<Schedule>) -> ScheduledDeploymentKeys {
	ScheduledDeploymentKeys::new(
		DeploymentId::parse("aidash://sender").unwrap(),
		DeploymentKeySource::File(initial.into()),
		store,
		Arc::new(FileKeySources),
	)
}

#[tokio::test]
async fn rollover_reads_shared_schedule_selects_source_at_activation_and_fails_closed() {
	let dir = tempfile::tempdir().unwrap();
	let old_path = dir.path().join("old.pem");
	let new_path = dir.path().join("new.pem");
	let old = FileDeploymentKey::generate(&old_path, false).unwrap();
	let new = FileDeploymentKey::generate(&new_path, false).unwrap();
	let state = Arc::new(Schedule::default());
	let first = ring(&old_path, state.clone());
	let second = ring(&old_path, state.clone());
	let now = 1_800_000_000;
	first
		.rollover(
			old.key_id(),
			DeploymentKeySource::File(new_path.clone()),
			now,
		)
		.await
		.unwrap();
	assert_eq!(
		second.signing(now).await.unwrap().key.key_id(),
		old.key_id()
	);
	let published = second.publication(now).await.unwrap();
	assert_eq!(published.current.id, old.key_id().to_string());
	assert_eq!(published.successor.unwrap().id, new.key_id().to_string());
	let active = now + 86_400;
	let selected = second.signing(active).await.unwrap();
	assert_eq!(selected.key.key_id(), new.key_id());
	assert!(selected.transition.is_some());
	assert!(
		first
			.rollover(
				old.key_id(),
				DeploymentKeySource::File(new_path.clone()),
				active
			)
			.await
			.is_err()
	);
	std::fs::remove_file(&old_path).unwrap();
	let restarted = ring(&old_path, state);
	assert_eq!(
		restarted
			.signing(active + 604_800)
			.await
			.unwrap()
			.transition,
		None
	);
	let published = restarted.publication(active + 604_800).await.unwrap();
	assert_eq!(published.current.id, new.key_id().to_string());
	assert!(published.retiring.is_none());
	std::fs::remove_file(&new_path).unwrap();
	assert!(
		restarted.signing(active).await.is_err(),
		"no predecessor fallback"
	);
	assert_eq!(
		restarted.publication(active).await.unwrap().current.id,
		new.key_id().to_string()
	);
}

#[tokio::test]
async fn rollover_rejects_missing_source_and_stale_current_without_publishing() {
	let dir = tempfile::tempdir().unwrap();
	let old_path = dir.path().join("old.pem");
	let new_path = dir.path().join("new.pem");
	let old = FileDeploymentKey::generate(&old_path, false).unwrap();
	let new = FileDeploymentKey::generate(&new_path, false).unwrap();
	let state = Arc::new(Schedule::default());
	let keys = ring(&old_path, state.clone());
	assert!(
		keys.rollover(
			old.key_id(),
			DeploymentKeySource::File(dir.path().join("missing")),
			1_800_000_000
		)
		.await
		.is_err()
	);
	assert!(
		keys.rollover(
			new.key_id(),
			DeploymentKeySource::File(new_path),
			1_800_000_000
		)
		.await
		.is_err()
	);
	assert!(state.load().await.unwrap().is_none());
}

#[tokio::test]
async fn changing_a_published_source_cannot_change_the_scheduled_identity() {
	let dir = tempfile::tempdir().unwrap();
	let old_path = dir.path().join("old.pem");
	let new_path = dir.path().join("new.pem");
	let old = FileDeploymentKey::generate(&old_path, false).unwrap();
	FileDeploymentKey::generate(&new_path, false).unwrap();
	let keys = ring(&old_path, Arc::new(Schedule::default()));
	let now = 1_800_000_000;
	keys.rollover(
		old.key_id(),
		DeploymentKeySource::File(new_path.clone()),
		now,
	)
	.await
	.unwrap();
	FileDeploymentKey::generate(&new_path, true).unwrap();
	assert!(keys.signing(now + 86_400).await.is_err());
	assert_eq!(keys.signing(now).await.unwrap().key.key_id(), old.key_id());
}

#[tokio::test]
async fn stored_schedule_must_match_the_signed_whole_second_boundaries() {
	let dir = tempfile::tempdir().unwrap();
	let old_path = dir.path().join("old.pem");
	let new_path = dir.path().join("new.pem");
	let old = FileDeploymentKey::generate(&old_path, false).unwrap();
	FileDeploymentKey::generate(&new_path, false).unwrap();
	let state = Arc::new(Schedule::default());
	let keys = ring(&old_path, state.clone());
	let now = 1_800_000_000;
	keys.rollover(old.key_id(), DeploymentKeySource::File(new_path), now)
		.await
		.unwrap();
	{
		let mut stored = state.0.lock().unwrap();
		let schedule = &mut stored.as_mut().unwrap().schedule;
		let fraction = chrono::Duration::milliseconds(500);
		schedule.published_at += fraction;
		schedule.activates_at += fraction;
		schedule.retires_at += fraction;
	}
	assert!(keys.signing(now + 86_400).await.is_err());
	assert!(keys.publication(now + 86_400).await.is_err());
}

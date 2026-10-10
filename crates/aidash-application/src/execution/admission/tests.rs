use super::*;
use crate::Error;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::Mutex;
struct Repository {
	remote: bool,
	fail: Option<&'static str>,
	local: Option<u64>,
	calls: Mutex<Vec<String>>,
	admitted: Mutex<Option<(Uuid, String, i64)>>,
}
#[fixture]
fn repository() -> Repository {
	Repository {
		remote: false,
		fail: None,
		local: Some(7),
		calls: Mutex::new(vec![]),
		admitted: Mutex::new(None),
	}
}
#[fixture]
fn request() -> ModelRequest {
	ModelRequest {
		instructions: "system".into(),
		context: json!({"text":"東京"}).into(),
		tools: vec![],
		max_output_tokens: 4096,
		content_parts: vec![],
		cache_scope: None,
	}
}
fn attempt() -> Uuid {
	Uuid::from_u128(1)
}
impl Repository {
	fn record(&self, name: &str) -> Result<()> {
		self.calls.lock().unwrap().push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
#[async_trait]
impl InferenceAdmissionRepository<u64> for Repository {
	fn remote(&self) -> bool {
		self.remote
	}
	async fn suspend(&self) -> Result<()> {
		self.record("suspend")
	}
	async fn admit_remote(&self, attempt: Uuid, digest: String, units: i64) -> Result<u64> {
		self.record("remote")?;
		*self.admitted.lock().unwrap() = Some((attempt, digest, units));
		Ok(9)
	}
	async fn reserve_local(&self, id: Uuid, window: usize, output: u32) -> Result<Option<u64>> {
		assert_eq!(id, attempt());
		assert_eq!(window, 131072);
		assert_eq!(output, 4096);
		self.record("local")?;
		Ok(self.local)
	}
}
#[rstest]
#[case::generated(Some(7))]
#[case::ordinary(None)]
#[tokio::test]
async fn local_inference_preserves_optional_quota_reservation_without_suspending_authority(
	mut repository: Repository,
	request: ModelRequest,
	#[case] reservation: Option<u64>,
) {
	repository.local = reservation;
	assert_eq!(
		reserve(&repository, attempt(), 131072, 4096, &request)
			.await
			.unwrap(),
		reservation
	);
	assert_eq!(*repository.calls.lock().unwrap(), vec!["local"]);
	assert_eq!(*repository.admitted.lock().unwrap(), None);
}
#[rstest]
#[tokio::test]
async fn home_admission_binds_attempt_digest_and_units_after_suspending_the_worker_scope(
	mut repository: Repository,
	request: ModelRequest,
) {
	repository.remote = true;
	assert_eq!(
		reserve(&repository, attempt(), 131072, 4096, &request)
			.await
			.unwrap(),
		Some(9)
	);
	assert_eq!(*repository.calls.lock().unwrap(), vec!["suspend", "remote"]);
	assert_eq!(
		*repository.admitted.lock().unwrap(),
		Some((attempt(), request.inference_digest(), 135168))
	);
}
#[rstest]
#[case::suspension("suspend", true)]
#[case::home("remote", true)]
#[case::local_quota("local", false)]
#[tokio::test]
async fn an_admission_fault_preserves_its_identity_without_switching_accounting_mode(
	mut repository: Repository,
	request: ModelRequest,
	#[case] boundary: &'static str,
	#[case] remote: bool,
) {
	repository.remote = remote;
	repository.fail = Some(boundary);
	let Error::Port(error) = reserve(&repository, attempt(), 131072, 4096, &request)
		.await
		.err()
		.unwrap()
	else {
		panic!("expected admission fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	assert_eq!(
		*repository.calls.lock().unwrap(),
		match boundary {
			"suspend" => vec!["suspend"],
			"remote" => vec!["suspend", "remote"],
			_ => vec!["local"],
		}
	);
	assert_eq!(*repository.admitted.lock().unwrap(), None);
}

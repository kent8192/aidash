use super::*;
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;
struct Transport {
	health: Value,
	fail: bool,
}
#[async_trait]
impl RunnerTransport for Transport {
	async fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
		assert_eq!(method, "GET");
		assert_eq!(path, "/v1/health");
		assert!(body.is_none());
		if self.fail {
			Err(Error::External("fixture failure".into()))
		} else {
			Ok(self.health.clone())
		}
	}
}
fn profile() -> HealthProfile {
	HealthProfile {
		image: "image".into(),
		runtime_class: "runsc".into(),
		cpu: 1,
		memory_bytes: 100,
		processes: 20,
		working_bytes: 200,
		temporary_bytes: 300,
	}
}
fn health() -> Value {
	json!({"protocol":"aidash-runner/1","verified":true,"python_verified":true,"image":"image","runtime_class":"runsc","instance":"current-runner","probe":{"resources":{"cpu":1,"memory_bytes":100,"swap_bytes":0,"processes":20,"working_bytes":200,"temporary_bytes":300}}})
}
#[rstest]
#[tokio::test]
async fn a_matching_probe_returns_the_original_runner_identity_and_observations() {
	let transport = Transport {
		health: health(),
		fail: false,
	};
	assert_eq!(
		verified_health(&transport, &profile(), true).await.unwrap(),
		health()
	);
}
#[rstest]
#[tokio::test]
async fn transport_failure_is_the_existing_actionable_runtime_unavailable_conflict() {
	let transport = Transport {
		health: health(),
		fail: true,
	};
	assert!(
		matches!(verified_health(&transport,&profile(),false).await,Err(Error::Conflict(message)) if message=="RUNTIME_UNAVAILABLE: start the runner and pass its deployment probes")
	);
}
#[rstest]
#[tokio::test]
async fn a_mismatched_probe_cannot_be_used_for_execution() {
	let mut health = health();
	health["image"] = json!("other");
	let transport = Transport {
		health,
		fail: false,
	};
	assert!(
		matches!(verified_health(&transport,&profile(),false).await,Err(Error::Conflict(message)) if message=="RUNTIME_UNAVAILABLE: deployment probes do not match the execution profile")
	);
}

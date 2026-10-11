//! Unit tests for serializers::core.
use super::*;
use serde_json::{Value, json};

/// The application profile `deploy/helm/aidash` renders with its default limits.
fn chart_profile() -> Value {
	json!({"admission":true,"cpu":2,"idle_seconds":1800,"install_seconds":300,"maximum_seconds":600,"memory_bytes":2147483648_u64,"operation_seconds":120,"output_bytes":8388608,"processes":128,"runner":{"endpoint":"http://app-execution-runner:8949","image":"reg/sandbox@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","namespace":"aidash-pr-1-sandbox","runtime_class":"aidash-gvisor-pr-1","token_env":"AIDASH_CORE_RUNNER_TOKEN"},"storage":"/var/lib/aidash/capabilities","temporary_bytes":268435456,"working_bytes":1073741824})
}

#[test]
fn chart_application_profile_is_a_valid_runner_profile_without_runner_only_fields() {
	let profile: Profile = serde_json::from_value(chart_profile()).unwrap();
	let runner = profile.runner.clone().unwrap();
	assert_eq!(runner.endpoint, "http://app-execution-runner:8949");
	assert_eq!(runner.token_env, "AIDASH_CORE_RUNNER_TOKEN");
	assert_eq!(runner.runtime_class, "aidash-gvisor-pr-1");
	assert!(runner.journal.is_none() && runner.listen_port.is_none());
	assert!(profile.admission);
	assert!(crate::capabilities::Runtime::new(profile).is_ok());
}

#[test]
fn a_profile_shared_with_the_runner_keeps_its_runner_only_fields() {
	let mut value = chart_profile();
	value["runner"]["journal"] = json!("/var/lib/aidash/journal");
	value["runner"]["kubectl"] = json!("/usr/local/bin/kubectl");
	value["runner"]["kubeconfig"] = json!("/etc/aidash/kubeconfig");
	value["runner"]["listen_host"] = json!("127.0.0.1");
	value["runner"]["listen_port"] = json!(8949);
	let runner = serde_json::from_value::<Profile>(value)
		.unwrap()
		.runner
		.unwrap();
	assert_eq!(runner.listen_port, Some(8949));
	assert_eq!(
		runner.journal.as_deref(),
		Some(std::path::Path::new("/var/lib/aidash/journal"))
	);
}

#[rstest::rstest]
#[case::sentry_pids_limit(&["host_tasks"], json!(512))]
#[case::runner_scheduling(&["runner", "node_selector"], json!({"pool": "execution"}))]
#[case::runner_endpoint_missing(&["runner", "endpoint"], Value::Null)]
fn runner_only_or_incomplete_profiles_are_rejected(#[case] path: &[&str], #[case] value: Value) {
	let mut profile = chart_profile();
	let (last, parents) = path.split_last().unwrap();
	let object = parents
		.iter()
		.fold(&mut profile, |node, key| &mut node[*key])
		.as_object_mut()
		.unwrap();
	if value.is_null() {
		object.remove(*last);
	} else {
		object.insert((*last).into(), value);
	}
	assert!(serde_json::from_value::<Profile>(profile).is_err());
}

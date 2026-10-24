use super::*;
use rstest::{fixture, rstest};
use serde_json::json;
#[fixture]
fn profile() -> HealthProfile {
	HealthProfile {
		image: "pinned-image".into(),
		runtime_class: "runsc".into(),
		cpu: 2,
		memory_bytes: 100,
		processes: 20,
		working_bytes: 4097,
		temporary_bytes: 300,
	}
}
#[fixture]
fn health() -> Value {
	json!({"protocol":"aidash-runner/1","verified":true,"python_verified":true,"image":"pinned-image","runtime_class":"runsc","probe":{"physical_page_size":4096,"resources":{"cpu":2,"memory_bytes":100,"swap_bytes":0,"processes":20,"working_bytes":8192,"temporary_bytes":300}}})
}
#[rstest]
fn matching_page_rounded_probes_allow_shell_and_python(profile: HealthProfile, health: Value) {
	assert!(profile.matches(&health, false));
	assert!(profile.matches(&health, true));
}
#[rstest]
#[case::protocol("/protocol",json!("other"))]
#[case::verification("/verified",json!(false))]
#[case::image("/image",json!("other"))]
#[case::runtime("/runtime_class",json!("other"))]
#[case::cpu("/probe/resources/cpu",json!(3))]
#[case::memory("/probe/resources/memory_bytes",json!(101))]
#[case::swap("/probe/resources/swap_bytes",json!(1))]
#[case::processes("/probe/resources/processes",json!(21))]
#[case::working("/probe/resources/working_bytes",json!(4097))]
#[case::temporary("/probe/resources/temporary_bytes",json!(301))]
fn an_unmatching_deployment_never_gains_execution_authority(
	profile: HealthProfile,
	mut health: Value,
	#[case] pointer: &str,
	#[case] value: Value,
) {
	*health.pointer_mut(pointer).unwrap() = value;
	assert!(!profile.matches(&health, false));
}
#[rstest]
fn python_probe_is_required_only_for_python_operations(profile: HealthProfile, mut health: Value) {
	health["python_verified"] = json!(false);
	assert!(profile.matches(&health, false));
	assert!(!profile.matches(&health, true));
}
#[rstest]
#[case::missing(Value::Null)]
#[case::zero(json!(0))]
fn unreported_page_size_keeps_the_original_exact_byte_fallback(
	profile: HealthProfile,
	mut health: Value,
	#[case] page: Value,
) {
	health["probe"]["physical_page_size"] = page;
	health["probe"]["resources"]["working_bytes"] = json!(4097);
	assert!(profile.matches(&health, false));
}
#[rstest]
#[case::round_up(4097, 4096, Some(8192))]
#[case::minimum(0, 4096, Some(4096))]
#[case::no_page(4097, 0, None)]
#[case::overflow(u64::MAX, 4096, None)]
fn physical_page_rounding_is_checked_and_preserves_the_minimum_page(
	#[case] bytes: u64,
	#[case] page: u64,
	#[case] expected: Option<u64>,
) {
	assert_eq!(rounded_working_bytes(bytes, page), expected);
}

use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn operation() -> OperationState {
	OperationState {
		area_id: Uuid::from_u128(1),
		run_id: Uuid::from_u128(2),
		principal: "saved-subject".into(),
		kind: "code_interpreter".into(),
		state: "prepared".into(),
	}
}
#[rstest]
#[case::same(1, 2, "saved-subject", "code_interpreter", true)]
#[case::area(3, 2, "saved-subject", "code_interpreter", false)]
#[case::run(1, 3, "saved-subject", "code_interpreter", false)]
#[case::principal(1, 2, "other-subject", "code_interpreter", false)]
#[case::kind(1, 2, "saved-subject", "shell", false)]
fn operation_disclosure_requires_same_area_run_principal_and_kind(
	operation: OperationState,
	#[case] area: u128,
	#[case] run: u128,
	#[case] principal: &str,
	#[case] kind: &str,
	#[case] visible: bool,
) {
	assert_eq!(
		operation.visible_to(Uuid::from_u128(area), Uuid::from_u128(run), principal, kind),
		visible
	);
}
#[rstest]
fn python_poll_accepts_an_install_without_loosening_other_kind_checks(
	mut operation: OperationState,
) {
	operation.kind = "python_install".into();
	assert!(operation.visible_to(
		operation.area_id,
		operation.run_id,
		&operation.principal,
		"code_interpreter"
	));
	assert!(!operation.visible_to(
		operation.area_id,
		operation.run_id,
		&operation.principal,
		"shell"
	));
	operation.kind = "code_interpreter".into();
	assert!(!operation.visible_to(
		operation.area_id,
		operation.run_id,
		&operation.principal,
		"python_install"
	));
}
#[rstest]
#[case::prepared("prepared", Some("cancelled"), true)]
#[case::submitted("submitted", Some("cancelling"), false)]
#[case::running("running", Some("cancelling"), false)]
#[case::cancelling("cancelling", Some("cancelling"), false)]
#[case::unknown("unknown", Some("cancelling"), false)]
#[case::completed("completed", None, false)]
#[case::cancelled("cancelled", None, false)]
#[case::failed("failed", None, false)]
#[case::withdrawn("withdrawn", None, false)]
fn cancellation_never_marks_a_dispatched_or_unknown_writer_as_safely_terminated(
	mut operation: OperationState,
	#[case] state: &str,
	#[case] next: Option<&str>,
	#[case] never_dispatched: bool,
) {
	operation.state = state.into();
	let change = operation.cancellation();
	assert_eq!(change.as_ref().map(|change| change.state), next);
	if let Some(change) = change {
		assert_eq!(change.never_dispatched, never_dispatched);
		assert_eq!(
			change.result,
			never_dispatched
				.then(|| json!({"termination_confirmed":true,"effects_may_have_occurred":false}))
		);
	}
}
#[fixture]
fn input() -> ShellRequest {
	ShellRequest {
		idempotency_key: Uuid::from_u128(1),
		command: "printf 'ok'".into(),
		timeout_seconds: None,
		expected_revision: 7,
	}
}
#[fixture]
fn limits() -> AdmissionLimits {
	AdmissionLimits {
		admission: true,
		working_bytes: 16,
		command_bytes: 16,
		operation_seconds: 10,
		maximum_seconds: 20,
	}
}
fn file(scope: FileScope, path: &str, size: u64) -> MountedFile {
	MountedFile {
		file_id: Uuid::from_u128(2),
		path: path.into(),
		digest: "immutable".into(),
		size,
		media_type: "text/plain".into(),
		scope,
		provenance: json!({"saved":true}),
	}
}
#[rstest]
#[case::default(None, 10)]
#[case::explicit(Some(20), 20)]
fn valid_shell_requests_keep_default_and_explicit_seconds(
	mut input: ShellRequest,
	limits: AdmissionLimits,
	#[case] seconds: Option<u64>,
	#[case] expected: u64,
) {
	input.timeout_seconds = seconds;
	assert_eq!(input.seconds(limits).unwrap(), expected);
}
#[rstest]
#[case::empty("", None)]
#[case::too_long("01234567890123456", None)]
#[case::zero("ok", Some(0))]
#[case::too_long_timeout("ok", Some(21))]
fn invalid_shell_bounds_keep_the_same_contract_code(
	mut input: ShellRequest,
	limits: AdmissionLimits,
	#[case] command: &str,
	#[case] seconds: Option<u64>,
) {
	input.command = command.into();
	input.timeout_seconds = seconds;
	assert!(
		matches!(input.seconds(limits),Err(Error::Invalid(code)) if code=="INVALID_SHELL_LIMIT")
	);
}
#[rstest]
fn operation_identity_binds_kind_run_and_complete_request_metadata(input: ShellRequest) {
	let run = Uuid::from_u128(3);
	let extra = json!({"session_id":Uuid::from_u128(4),"package_files":[]});
	assert_eq!(
		input.digest("code_interpreter", run, &extra),
		crate::registry::rules::digest(&json!(["code_interpreter",run,{
        "idempotency_key":Uuid::from_u128(1),"command":"printf 'ok'","timeout_seconds":null,"expected_revision":7
    },extra]))
	);
	assert_eq!(input.key(run), format!("core:{run}:{}", Uuid::from_u128(1)));
	assert_ne!(
		input.digest("shell", run, &extra),
		input.digest("code_interpreter", run, &extra)
	);
	assert_ne!(
		input.digest("shell", run, &extra),
		input.digest("shell", Uuid::from_u128(5), &extra)
	);
	assert_ne!(
		input.digest("shell", run, &extra),
		input.digest("shell", run, &json!({}))
	);
}
#[rstest]
fn mounted_file_paths_collide_only_within_the_same_scope() {
	assert!(has_input_path_collision(&[
		file(FileScope::References, "same", 1),
		file(FileScope::References, "same", 1)
	]));
	assert!(!has_input_path_collision(&[
		file(FileScope::References, "same", 1),
		file(FileScope::Working, "same", 1),
		file(FileScope::Received, "same", 1)
	]));
}
#[rstest]
#[case::working_exact(vec![file(FileScope::Working,"w",16)],None)]
#[case::too_big(vec![file(FileScope::Working,"w",17)],Some("WORKING_QUOTA_EXCEEDED"))]
#[case::readonly_full(vec![file(FileScope::References,"r",16)],Some("WORKING_QUOTA_EXCEEDED"))]
#[case::some_writable(vec![file(FileScope::References,"r",15)],None)]
#[case::collision_before_quota(vec![file(FileScope::Received,"r",17),file(FileScope::Received,"r",17)],Some("INPUT_PATH_COLLISION"))]
#[case::overflow(vec![file(FileScope::Working,"a",u64::MAX),file(FileScope::Working,"b",1)],Some("WORKING_QUOTA_EXCEEDED"))]
fn mounted_inputs_preserve_quota_boundaries_and_collision_precedence(
	#[case] files: Vec<MountedFile>,
	#[case] code: Option<&str>,
) {
	assert_eq!(
		request_validation_failure(16, &files).map(|failure| failure.0),
		code
	);
}
#[rstest]
fn package_files_preserve_all_metadata_and_reject_incomplete_or_wrong_scope_input() {
	let mounted = file(FileScope::References, "package.whl", 7);
	let encoded = serde_json::to_value(&mounted).unwrap();
	let decoded = package_files(&json!({"package_files":[encoded]})).unwrap();
	assert_eq!(serde_json::to_value(&decoded[0]).unwrap(), encoded);
	assert!(package_files(&json!({"package_files":[{"path":"missing"}]})).is_err());
	assert!(package_files(&json!({"package_files":[{"scope":"unknown"}]})).is_err());
	assert_eq!(package_files(&json!({})).unwrap().len(), 0);
}
#[rstest]
#[case::active("active", None)]
#[case::running("running", Some("AREA_BUSY"))]
#[case::uncertain("uncertain", Some("AREA_UNAVAILABLE"))]
#[case::unknown("unknown", Some("AREA_UNAVAILABLE"))]
fn area_availability_keeps_busy_distinct_from_unavailable(
	#[case] state: &str,
	#[case] code: Option<&str>,
) {
	match code {
		None => available(state).unwrap(),
		Some(code) => {
			assert!(matches!(available(state),Err(Error::Conflict(actual)) if actual==code))
		}
	}
}

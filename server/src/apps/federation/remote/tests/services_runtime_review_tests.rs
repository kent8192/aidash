use super::*;

#[test]
fn remote_attachment_rejects_changed_bytes_size_digest_and_message() {
	use base64::Engine as _;
	let message = Uuid::from_u128(7);
	let bytes = b"immutable";
	let digest = crate::apps::execution::repositories::capability_objects::digest(bytes);
	let valid = json!({"filename":"a.png","media_type":"image/png","message_id":message,"size_bytes":bytes.len(),"sha256":digest,"content_base64":base64::engine::general_purpose::STANDARD.encode(bytes)});
	assert_eq!(
		decode_thread_attachment(valid.clone(), message, &digest)
			.unwrap()
			.2,
		bytes
	);
	for (field, value) in [
		("size_bytes", json!(1)),
		("sha256", json!("wrong")),
		("message_id", json!(Uuid::nil())),
		("content_base64", json!("eA==")),
	] {
		let mut changed = valid.clone();
		changed[field] = value;
		assert!(
			matches!(
				decode_thread_attachment(changed, message, &digest),
				Err(Error::Conflict(_))
			),
			"{field}"
		);
	}
}

#[rstest::rstest]
fn child_task_summary_is_independent_of_child_payload_sizes() {
	let mut summary = ChildTaskSummary {
		has_pending: false,
		has_failed: false,
	};
	for status in [
		crate::domain::TaskStatus::Running,
		crate::domain::TaskStatus::Failed,
	] {
		summary.include_status(status);
	}
	assert!(summary.has_pending && summary.has_failed);
	assert!(serde_json::to_vec(&summary).unwrap().len() < 64);
}

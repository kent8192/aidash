use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn upload() -> Upload {
	Upload {
		idempotency_key: Uuid::from_u128(1),
		name: "reference.txt".into(),
		media_type: "text/plain".into(),
		size: 7,
		digest: "a".repeat(64),
	}
}

#[rstest]
#[case::zero("size")]
#[case::path("path")]
#[case::control("control")]
#[case::uppercase_digest("digest")]
#[case::unsupported_media("media")]
fn invalid_originals_are_rejected_with_the_existing_upload_code(
	mut upload: Upload,
	#[case] changed: &str,
) {
	match changed {
		"size" => upload.size = 0,
		"path" => upload.name = "../reference.txt".into(),
		"control" => upload.name = "reference\n.txt".into(),
		"digest" => upload.digest = "A".repeat(64),
		"media" => upload.media_type = "application/octet-stream".into(),
		_ => panic!("unknown fixture"),
	}
	assert!(
		matches!(validate_upload(&upload,16),Err(Error::Invalid(message)) if message=="REFERENCE_UPLOAD_LIMIT")
	);
}

#[rstest]
fn upload_size_and_name_boundaries_do_not_change(upload: Upload) {
	assert!(validate_upload(&upload, 7).is_ok());
	assert!(validate_upload(&upload, 6).is_err());
	let mut input = upload;
	input.name = "a".repeat(255);
	assert!(validate_upload(&input, 7).is_ok());
	input.name.push('a');
	assert!(validate_upload(&input, 7).is_err());
}

#[rstest]
#[case::single_final(0, 3, 3, true)]
#[case::short_final(0, 3, 2, false)]
#[case::final_after_full(4<<20,(4<<20)+3,3,true)]
#[case::empty(0, 3, 0, false)]
#[case::past_end(4, 3, 1, false)]
#[case::full_chunk(0,(4<<20)+3,4<<20,true)]
#[case::oversized(0,(4<<20)+3,(4<<20)+1,false)]
fn chunk_admission_requires_exact_sequential_block_sizes(
	#[case] offset: u64,
	#[case] maximum: u64,
	#[case] size: usize,
	#[case] accepted: bool,
) {
	assert_eq!(valid_chunk(offset, maximum, &vec![0; size]), accepted);
}

#[fixture]
fn record(upload: Upload) -> Record {
	Record {
		id: Uuid::from_u128(2),
		tenant: "tenant".into(),
		owner: "owner".into(),
		area_id: None,
		kind: "reference".into(),
		state: "extracting".into(),
		revision: 3,
		data: json!({"input":upload,"chunks":[],"uploaded_bytes":7,"original":null,"extraction":null}),
		expires_at: None,
	}
}

#[rstest]
fn parser_budget_is_immutable_after_the_dispatch_intent(mut record: Record) {
	assert_eq!(text_budget(&record, 1000, 856), 100);
	assert_eq!(text_budget(&record, 1000, 255), 0);
	record.data["text_budget"] = json!(47);
	assert_eq!(text_budget(&record, 1, 0), 47);
}

#[rstest]
#[case::ready("ready", true)]
#[case::text_limit("text_limit", true)]
#[case::encrypted("encrypted", true)]
#[case::expanded_limit("expanded_size_limit", true)]
#[case::running("running", false)]
#[case::unknown("unknown", false)]
fn only_the_existing_parser_result_states_can_be_published(
	#[case] state: &str,
	#[case] supported: bool,
) {
	assert_eq!(extraction_state_supported(state), supported);
}

#[rstest]
fn parser_completion_preserves_the_original_and_schedules_only_a_receipt(mut record: Record) {
	let original = json!({"binding":"immutable"});
	record.data["original"] = original.clone();
	finish_extraction(&mut record, "encrypted", "runner-digest");
	assert_eq!(record.state, "ready");
	assert!(record.expires_at.is_none());
	assert_eq!(record.data["original"], original);
	assert_eq!(record.data["receipt_pending"], true);
	assert_eq!(record.data["runner_digest"], "runner-digest");
	assert_eq!(record.data["extraction_state"], "encrypted");
	assert_eq!(record.revision, 3);
}

#[rstest]
fn receipt_pages_advance_over_failed_rows_and_wrap_at_the_end() {
	let page = vec![
		(Uuid::from_u128(1), json!({})),
		(Uuid::from_u128(2), json!({})),
	];
	assert_eq!(next_receipt_cursor(&page), Uuid::from_u128(2));
	assert_eq!(next_receipt_cursor(&[]), Uuid::nil());
}

#[rstest]
fn the_reference_view_keeps_original_and_extraction_metadata_nullable(record: Record) {
	let value = view(&record).unwrap();
	assert_eq!(value.reference_id, record.id);
	assert_eq!(value.revision, 3);
	assert_eq!(value.uploaded_bytes, 7);
	assert!(value.original.is_none());
	assert!(value.extraction.is_none());
	assert!(value.extraction_state.is_none());
}

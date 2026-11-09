use super::*;
use async_trait::async_trait;
use uuid::Uuid;
struct Reader {
	bytes: Vec<u8>,
	limit: usize,
	reads: usize,
}
#[async_trait]
impl OutputReader for Reader {
	fn read_bytes(&self) -> Result<usize> {
		Ok(self.limit)
	}
	async fn read(&mut self, _: &FileEntry) -> Result<Vec<u8>> {
		self.reads += 1;
		Ok(self.bytes.clone())
	}
}
fn view<'a>(state: &'a str, input: &'a Value, result: &'a Value) -> OperationView<'a> {
	OperationView {
		id: Uuid::new_v4(),
		area_id: Uuid::new_v4(),
		kind: "shell",
		state,
		generation: 3,
		revision: 5,
		epoch: 7,
		policy_revision: 11,
		input,
		result,
	}
}
fn output_file() -> Value {
	json!({"file_id":Uuid::new_v4(),"path":"output.txt","digest":"digest","size":6,"media_type":"text/plain","scope":"working","provenance":null})
}
#[tokio::test]
async fn polling_at_the_output_end_omits_unchanged_output_and_keeps_the_cursor() {
	let data = json!({"output_file": output_file()});
	let input = json!({});
	let mut reader = Reader {
		bytes: "ééab".as_bytes().to_vec(),
		limit: 3,
		reads: 0,
	};
	let page = result(&mut reader, &view("running", &input, &data), 4)
		.await
		.unwrap();
	let page = serde_json::to_value(page).unwrap();
	assert_eq!(page["output"], "ab");
	assert_eq!(page["next_offset"], 6);
	let unchanged = result(&mut reader, &view("running", &input, &data), 6)
		.await
		.unwrap();
	let unchanged = serde_json::to_value(unchanged).unwrap();
	assert!(unchanged.get("output").is_none());
	assert_eq!(unchanged["output_unchanged"], true);
	assert_eq!(unchanged["next_offset"], 6);
}
#[tokio::test]
async fn ended_operations_only_offer_a_cursor_when_stored_output_remains() {
	let data = json!({"output_file": output_file()});
	let input = json!({});
	let mut reader = Reader {
		bytes: "ééab".as_bytes().to_vec(),
		limit: 3,
		reads: 0,
	};
	let first = result(&mut reader, &view("completed", &input, &data), 0)
		.await
		.unwrap();
	assert_eq!(first.next_offset, Some(2));
	let last = result(&mut reader, &view("completed", &input, &data), 4)
		.await
		.unwrap();
	assert_eq!(last.output, "ab");
	assert_eq!(last.next_offset, None);
	assert!(!last.output_unchanged);
	let repeated = result(&mut reader, &view("completed", &input, &data), 6)
		.await
		.unwrap();
	assert_eq!(repeated.next_offset, None);
	assert!(repeated.output_unchanged);
	let empty = result(&mut reader, &view("completed", &input, &json!({})), 0)
		.await
		.unwrap();
	assert_eq!(empty.next_offset, None);
	assert!(
		!empty.output_unchanged,
		"a first empty result is not a repeated poll"
	);
}

#[tokio::test]
async fn the_deadline_of_a_running_command_counts_from_its_start() {
	let input = json!({"seconds":600});
	let data = json!({"preview":"","started_at":1000.5});
	let mut reader = Reader {
		bytes: vec![],
		limit: 16,
		reads: 0,
	};
	let page = result(&mut reader, &view("running", &input, &data), 0)
		.await
		.unwrap();
	assert_eq!(
		page.operation_deadline,
		chrono::DateTime::from_timestamp_millis(1_600_500)
	);
}

#[rstest::rstest]
#[case::not_started("submitted", json!({"preview":""}))]
#[case::ended("failed", json!({"preview":"","exit_code":143}))]
#[tokio::test]
async fn a_command_that_is_not_running_has_no_deadline(#[case] state: &str, #[case] data: Value) {
	let input = json!({"seconds":600});
	let mut reader = Reader {
		bytes: vec![],
		limit: 16,
		reads: 0,
	};
	let page = result(&mut reader, &view(state, &input, &data), 0)
		.await
		.unwrap();
	assert_eq!(page.operation_deadline, None);
}

#[tokio::test]
async fn every_active_operation_retains_its_cursor_at_the_current_end() {
	let input = json!({});
	let data = json!({"preview":"a"});
	let mut reader = Reader {
		bytes: vec![],
		limit: 16,
		reads: 0,
	};
	for state in ["prepared", "submitted", "running", "cancelling"] {
		let page = result(&mut reader, &view(state, &input, &data), 1)
			.await
			.unwrap();
		assert_eq!(page.next_offset, Some(1), "{state}");
		assert!(page.output_unchanged, "{state}");
	}
}

#[tokio::test]
async fn a_sliding_running_preview_uses_absolute_output_offsets() {
	let input = json!({});
	let mut reader = Reader {
		bytes: vec![],
		limit: 16,
		reads: 0,
	};
	let first = json!({"preview":"abc", "preview_offset":4});
	let page = result(&mut reader, &view("running", &input, &first), 0)
		.await
		.unwrap();
	assert_eq!(page.output, "abc");
	assert_eq!(page.next_offset, Some(7));
	let later = json!({"preview":"cde", "preview_offset":6});
	let page = result(&mut reader, &view("running", &input, &later), 7)
		.await
		.unwrap();
	assert_eq!(page.output, "de");
	assert_eq!(page.next_offset, Some(9));
	let unchanged = result(&mut reader, &view("running", &input, &later), 9)
		.await
		.unwrap();
	assert!(unchanged.output_unchanged);
	assert_eq!(unchanged.next_offset, Some(9));
}

#[tokio::test]
async fn lossy_preview_keeps_offsets_in_original_output_bytes() {
	let input = json!({});
	let data = json!({"preview":"�x", "preview_bytes":"/3g=", "preview_offset":4});
	let mut reader = Reader {
		bytes: vec![],
		limit: 16,
		reads: 0,
	};
	let page = result(&mut reader, &view("running", &input, &data), 4)
		.await
		.unwrap();
	assert_eq!(page.output, "�x");
	assert_eq!(page.next_offset, Some(6));
	let unchanged = result(&mut reader, &view("running", &input, &data), 6)
		.await
		.unwrap();
	assert!(unchanged.output_unchanged);
}

#[rstest::rstest]
#[tokio::test]
async fn utf8_output_pages_preserve_complete_character_boundaries() {
	let data =
		json!({"output_file":output_file(),"termination_confirmed":true,"writer_frozen":true});
	let input = json!({});
	let op = view("completed", &input, &data);
	let mut reader = Reader {
		bytes: "ééab".as_bytes().to_vec(),
		limit: 3,
		reads: 0,
	};
	let result = result(&mut reader, &op, 0).await.unwrap();
	assert_eq!(result.output, "é");
	assert_eq!(result.next_offset, Some(2));
	assert!(result.writer_frozen);
	assert!(result.termination_confirmed);
	assert_eq!(reader.reads, 1);
}
#[rstest::rstest]
#[tokio::test]
async fn an_offset_inside_a_character_is_rejected() {
	let data = json!({"output_file":output_file()});
	let input = json!({});
	let mut reader = Reader {
		bytes: "ééab".as_bytes().to_vec(),
		limit: 3,
		reads: 0,
	};
	assert!(
		matches!(result(&mut reader,&view("completed",&input,&data),1).await,Err(Error::Invalid(code)) if code=="INVALID_READ_RANGE")
	);
}
#[rstest::rstest]
#[case("prepared",json!({}),false)]
#[case("completed",json!({}),true)]
#[case("completed",json!({"effects_may_have_occurred":false}),false)]
#[tokio::test]
async fn durable_uncertainty_is_projected_without_reading_bytes(
	#[case] state: &str,
	#[case] data: Value,
	#[case] effects: bool,
) {
	let input = json!({});
	let mut reader = Reader {
		bytes: vec![],
		limit: 3,
		reads: 0,
	};
	let result = result(&mut reader, &view(state, &input, &data), 0)
		.await
		.unwrap();
	assert_eq!(result.effects_may_have_occurred, effects);
	assert!(
		serde_json::to_value(&result)
			.unwrap()
			.get("output_stale_seconds")
			.is_none()
	);
	assert_eq!(reader.reads, 0);
}

#[tokio::test]
async fn polling_exposes_preview_age_and_the_runners_error_message() {
	let input = json!({});
	let data = json!({"output_fetched_at":chrono::Utc::now().timestamp() - 12,
		"error":{"code":"OUTPUT_LOST","message":"File changes were not saved."}});
	let mut reader = Reader {
		bytes: vec![],
		limit: 3,
		reads: 0,
	};
	let projected = result(&mut reader, &view("uncertain", &input, &data), 0)
		.await
		.unwrap();
	assert!(projected.output_stale_seconds.unwrap() >= 12);
	assert_eq!(
		projected.error.unwrap().message,
		"File changes were not saved."
	);
}

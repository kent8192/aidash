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
	assert_eq!(reader.reads, 0);
}

use super::*;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

#[test]
fn a_path_a_source_would_refuse_is_not_reported_as_added() {
	assert_eq!(
		prepare_batch_file("plan/api.md.extracted.txt", "aGk=", 1024),
		Err("INVALID_PATH")
	);
	assert_eq!(
		prepare_batch_file("plan/api.md", "aGk=", 1024),
		Ok(b"hi".to_vec())
	);
}

struct Receipts {
	rows: Vec<(Uuid, Value)>,
	acknowledged: Mutex<Vec<Uuid>>,
}
#[async_trait]
impl ReferenceRepository for Receipts {
	async fn snapshot(&self, _: Uuid) -> Result<Record> {
		unreachable!("no active reference work")
	}
	async fn begin(&self, _: &Record) -> Result<Box<dyn ReferenceScope + '_>> {
		unreachable!("no active reference work")
	}
	async fn active(&self, _: Uuid) -> Result<Vec<Uuid>> {
		Ok(vec![])
	}
	async fn receipts(&self, _: Uuid) -> Result<Vec<(Uuid, Value)>> {
		Ok(self.rows.clone())
	}
	async fn acknowledge(&self, id: Uuid) -> Result<()> {
		self.acknowledged.lock().unwrap().push(id);
		Ok(())
	}
	async fn request(&self, _: &str, _: &str, _: Option<Value>) -> Result<Value> {
		Ok(Value::Null)
	}
}

/// #728: this row stopped the process, and again after each restart.
#[tokio::test]
async fn a_receipt_that_cannot_be_read_does_not_hold_back_the_rest_of_the_batch() {
	// Arrange
	let (unreadable, readable) = (Uuid::from_u128(1), Uuid::from_u128(2));
	let repository = Receipts {
		rows: vec![
			(unreadable, json!({"operation_id": "not a uuid"})),
			(
				readable,
				json!({"operation_id": Uuid::from_u128(3), "runner_digest": "digest"}),
			),
		],
		acknowledged: Mutex::default(),
	};
	// Act
	sweep(&repository, &mut Cursor::default()).await.unwrap();
	// Assert
	assert_eq!(*repository.acknowledged.lock().unwrap(), [readable]);
}

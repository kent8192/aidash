use super::*;
use crate::ports::capabilities::transfer::{Limits, ReceiptScope};
use aidash_domain::capabilities::{operations::FileScope, sharing::Recipient};
use async_trait::async_trait;
use std::sync::Mutex;

struct Repository {
	record: Record,
	receipt: Value,
	events: Mutex<Vec<String>>,
	fail_update: bool,
}
struct Receipt<'a>(&'a Repository);
#[async_trait]
impl TransferRepository for Repository {
	fn limits(&self) -> Limits {
		Limits {
			admission: false,
			share_files: 16,
			share_file_bytes: 10,
			share_bytes: 10,
			staging_seconds: 60,
			working_bytes: 10,
		}
	}
	async fn snapshot(&self, _: Uuid) -> Result<Record> {
		Ok(self.record.clone())
	}
	async fn begin_sender(&self, _: &Record) -> Result<Box<dyn TransferScope + '_>> {
		self.events.lock().unwrap().push("authority".into());
		Err(Error::Forbidden)
	}
	async fn begin_receipt(&self, _: Uuid) -> Result<Box<dyn ReceiptScope + '_>> {
		Ok(Box::new(Receipt(self)))
	}
	async fn request(&self, _: &str, path: &str, _: &Value) -> Result<Value> {
		self.events.lock().unwrap().push(path.into());
		Ok(self.receipt.clone())
	}
	async fn jobs(&self) -> Result<Vec<Uuid>> {
		Ok(vec![])
	}
	async fn record_failure(&self, _: Uuid, _: bool) -> Result<()> {
		Err(Error::External("unexpected retry".into()))
	}
}
#[async_trait]
impl ReceiptScope for Receipt<'_> {
	async fn load(&mut self) -> Result<Record> {
		Ok(self.0.record.clone())
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		assert_eq!(record.state, "delivered");
		assert!(record.data["error"].is_null());
		self.0.events.lock().unwrap().push("update".into());
		if self.0.fail_update {
			Err(Error::External("write failed".into()))
		} else {
			Ok(())
		}
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.0
			.events
			.lock()
			.unwrap()
			.push(if result.is_ok() { "commit" } else { "rollback" }.into());
		result
	}
}
fn repository() -> Repository {
	let id = Uuid::new_v4();
	let description = Description {
		protocol: "file-transfer/1".into(),
		transfer_id: id,
		source_node: "source".into(),
		target: Recipient {
			node_id: "target".into(),
			agent_id: "recipient".into(),
			agent_version: "v1".into(),
			thread_id: Uuid::new_v4(),
		},
		source_tenant: "tenant".into(),
		source_subject: "owner".into(),
		source_agent: EntityRef {
			id: "sender".into(),
			version: "v1".into(),
		},
		input_digest: "input".into(),
		manifest_digest: "manifest".into(),
		files: vec![FileEntry {
			file_id: Uuid::new_v4(),
			path: "report.txt".into(),
			digest: "digest".into(),
			size: 5,
			media_type: "text/plain".into(),
			scope: FileScope::Working,
			provenance: Value::Null,
		}],
		expires_at: Utc::now() - Duration::hours(1),
	};
	let receipt = json!({"state":"committed","transfer_id":id,"input_digest":"input","manifest_digest":"manifest","receipt":{"node_id":"target","files":[{"path":format!("{id}/report.txt"),"digest":"digest","size":5}]}});
	let record = Record {
		id,
		tenant: "tenant".into(),
		owner: "owner".into(),
		area_id: Some(Uuid::new_v4()),
		kind: "transfer_out".into(),
		state: "committing".into(),
		revision: 2,
		data: json!({"description":description,"prepared":true,"commit_attempted":true,"error":"lost reply"}),
		expires_at: Some(description.expires_at),
	};
	Repository {
		record,
		receipt,
		events: Mutex::new(vec![]),
		fail_update: false,
	}
}
#[rstest::rstest]
#[tokio::test]
async fn lost_reply_is_reconciled_after_expiry_without_new_authority_or_effects() {
	let repository = repository();
	drive(&repository, repository.record.id).await.unwrap();
	assert_eq!(
		*repository.events.lock().unwrap(),
		vec!["/scoped/files/status", "update", "commit"]
	);
}
#[rstest::rstest]
#[tokio::test]
async fn an_invalid_receipt_cannot_commit_completion() {
	let repository = repository();
	let mut receipt = repository.receipt.clone();
	receipt["receipt"]["node_id"] = json!("other");
	assert!(
		matches!(delivered(&repository, repository.record.id, receipt).await, Err(Error::Conflict(code)) if code == "INVALID_TRANSFER_RECEIPT")
	);
	assert_eq!(*repository.events.lock().unwrap(), vec!["rollback"]);
}
#[rstest::rstest]
#[tokio::test]
async fn a_failed_completion_update_rolls_back() {
	let mut repository = repository();
	repository.fail_update = true;
	assert!(
		matches!(delivered(&repository, repository.record.id, repository.receipt.clone()).await, Err(Error::External(code)) if code == "write failed")
	);
	assert_eq!(
		*repository.events.lock().unwrap(),
		vec!["update", "rollback"]
	);
}

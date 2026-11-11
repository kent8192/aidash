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

/// A scope for one Reference whose uploader is "alice". `principal` is the
/// caller and `granted` the actions the Tenant's policy gives that caller on it.
struct One {
	principal: &'static str,
	granted: &'static [&'static str],
	record: Record,
	updated: Option<Record>,
}
impl One {
	fn new(principal: &'static str, granted: &'static [&'static str]) -> Self {
		Self {
			principal,
			granted,
			record: Record {
				id: Uuid::from_u128(7),
				tenant: "acme".into(),
				owner: "alice".into(),
				area_id: None,
				kind: "reference".into(),
				state: "ready".into(),
				revision: 3,
				data: json!({}),
				expires_at: None,
			},
			updated: None,
		}
	}
}
#[async_trait]
impl ReferenceScope for One {
	fn principal(&self) -> &str {
		self.principal
	}
	fn identity(&self) -> Value {
		unreachable!()
	}
	fn limits(&self) -> Result<crate::ports::capabilities::references::Limits> {
		unreachable!()
	}
	fn mounts(&self) -> Result<crate::ports::capabilities::references::Mounts> {
		unreachable!()
	}
	fn set_constraints(&mut self, _: Value) -> Result<()> {
		unreachable!()
	}
	fn set_manifest(&mut self, _: Value) -> Result<()> {
		unreachable!()
	}
	async fn load(&mut self, _: Uuid) -> Result<Record> {
		Ok(self.record.clone())
	}
	async fn load_many(&mut self, _: &[Uuid]) -> Result<Vec<Record>> {
		Ok(vec![self.record.clone()])
	}
	async fn find_existing(&mut self, _: &str) -> Result<Option<Record>> {
		unreachable!()
	}
	async fn require(&mut self, _: Uuid, owner: &str, action: &str) -> Result<()> {
		assert_eq!(owner, "alice", "the policy sees who uploaded the file");
		if self.granted.contains(&action) {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn require_upload(&mut self) -> Result<()> {
		unreachable!()
	}
	async fn cached(&mut self, _: Uuid, _: &str) -> Result<Option<Value>> {
		unreachable!()
	}
	async fn cache(&mut self, _: Uuid, _: &str, _: &Value) -> Result<()> {
		unreachable!()
	}
	async fn insert(
		&mut self,
		_: Uuid,
		_: &str,
		_: Value,
		_: Option<chrono::DateTime<Utc>>,
	) -> Result<Record> {
		unreachable!()
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		self.updated = Some(record.clone());
		Ok(())
	}
	async fn put(&mut self, _: &str, _: &[u8]) -> Result<(Uuid, String)> {
		unreachable!()
	}
	async fn read(&mut self, _: &FileEntry) -> Result<Vec<u8>> {
		unreachable!()
	}
	async fn read_chunk(&mut self, _: &FileEntry, _: u64) -> Result<Vec<u8>> {
		unreachable!()
	}
	async fn begin_original(&mut self, _: u64) -> Result<()> {
		unreachable!()
	}
	async fn write_original(&mut self, _: &[u8]) -> Result<()> {
		unreachable!()
	}
	async fn finish_original(&mut self, _: &str) -> Result<(Uuid, String)> {
		unreachable!()
	}
	async fn publish(&mut self) -> Result<()> {
		unreachable!()
	}
	async fn verified_health(&mut self) -> Result<Value> {
		unreachable!()
	}
	async fn request(&mut self, _: &str, _: &str, _: Option<Value>) -> Result<Value> {
		unreachable!()
	}
	async fn finish(self: Box<Self>, _: Result<()>) -> Result<()> {
		unreachable!()
	}
}

/// #826: the uploader may be gone, so the Tenant's policy decides who revokes.
#[tokio::test]
async fn a_subject_granted_reference_delete_revokes_a_file_it_did_not_upload() {
	// Arrange
	let mut scope = One::new("bob", &["reference.delete"]);
	// Act
	revoke(&mut scope, Uuid::from_u128(7), 3).await.unwrap();
	// Assert
	let updated = scope.updated.unwrap();
	assert_eq!(updated.state, "revoked");
	assert!(updated.expires_at.is_some());
}

#[tokio::test]
async fn a_subject_without_reference_delete_gets_not_found_for_a_file_it_did_not_upload() {
	// Arrange
	let mut scope = One::new("bob", &["reference.read"]);
	// Act
	let refused = revoke(&mut scope, Uuid::from_u128(7), 3).await;
	// Assert
	assert!(matches!(refused, Err(Error::NotFound(_))), "{refused:?}");
	assert!(scope.updated.is_none());
}

#[tokio::test]
async fn a_subject_granted_only_reference_delete_cannot_read_the_file() {
	// Arrange
	let mut scope = One::new("bob", &["reference.delete", "reference.read"]);
	// Act
	let read = inspect(&mut scope, Uuid::from_u128(7)).await;
	// Assert: even a read grant stops at the uploader.
	assert!(matches!(read, Err(Error::NotFound(_))), "{read:?}");
}

#[tokio::test]
async fn revoking_with_a_stale_revision_conflicts() {
	let mut scope = One::new("bob", &["reference.delete"]);
	let stale = revoke(&mut scope, Uuid::from_u128(7), 2).await;
	assert!(matches!(stale, Err(Error::Conflict(_))), "{stale:?}");
}

#[tokio::test]
async fn a_revoked_file_cannot_be_revoked_again() {
	let mut scope = One::new("bob", &["reference.delete"]);
	scope.record.state = "revoked".into();
	let again = revoke(&mut scope, Uuid::from_u128(7), 3).await;
	assert!(matches!(again, Err(Error::NotFound(_))), "{again:?}");
}

#[tokio::test]
async fn the_revocation_view_needs_reference_delete_and_names_the_revision() {
	let mut allowed = One::new("bob", &["reference.delete"]);
	let record = revocation(&mut allowed, Uuid::from_u128(7)).await.unwrap();
	assert_eq!((record.revision, record.state.as_str()), (3, "ready"));
	let mut refused = One::new("bob", &["reference.read"]);
	let refused = revocation(&mut refused, Uuid::from_u128(7)).await;
	assert!(matches!(refused, Err(Error::NotFound(_))), "{refused:?}");
}

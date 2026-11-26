use super::*;
use aidash_domain::capabilities::sessions::Area;
use async_trait::async_trait;
use rstest::rstest;
use std::collections::BTreeSet;

struct Fake {
	/// Actions the Tenant's policy allows, by `kind:action`.
	allowed: BTreeSet<String>,
	asked: Vec<String>,
}
#[async_trait]
impl DeletionAuthority for Fake {
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let key = format!("{}:{action}", resource.kind);
		self.asked.push(key.clone());
		Ok(self.allowed.contains(&key))
	}
	async fn node_resource(&mut self, _thread: Uuid) -> Result<Resource> {
		Ok(Resource {
			tenant: "acme".into(),
			kind: "node".into(),
			id: "brave-otter-k3m9q2xa".into(),
			attributes: json!({}),
		})
	}
}
fn thread(owner: &str) -> Resource {
	Resource {
		tenant: "acme".into(),
		kind: "thread".into(),
		id: Uuid::nil().to_string(),
		attributes: json!({"owner": owner, "thread_id": Uuid::nil(), "duty_id": null}),
	}
}
fn fake(allowed: &[&str]) -> Fake {
	Fake {
		allowed: allowed.iter().map(|a| (*a).to_owned()).collect(),
		asked: vec![],
	}
}

#[rstest]
#[case::owner_with_delete("alice", &["thread:thread.delete"], true)]
#[case::owner_without_delete("alice", &[], false)]
#[case::reader_with_delete("bob", &["thread:thread.delete"], false)]
#[case::reader("bob", &["thread:thread.read"], false)]
#[case::node_administrator("bob", &["node:node.administer"], true)]
#[tokio::test]
async fn only_the_owner_or_a_node_administrator_deletes_a_thread(
	#[case] principal: &str,
	#[case] allowed: &[&str],
	#[case] expected: bool,
) {
	// Arrange: Alice owns the Thread.
	let mut scope = fake(allowed);
	// Act
	let permitted = may_delete(&mut scope, principal, &thread("alice"), Uuid::nil())
		.await
		.unwrap();
	// Assert
	assert_eq!(permitted, expected);
}

#[tokio::test]
async fn a_reader_holding_thread_delete_is_never_asked_for_it() {
	// Arrange: Bob reads Alice's Thread and holds `thread.delete` broadly.
	let mut scope = fake(&["thread:thread.delete", "thread:thread.read"]);
	// Act
	let permitted = may_delete(&mut scope, "bob", &thread("alice"), Uuid::nil())
		.await
		.unwrap();
	// Assert: only the Node administration decision counts for a non-owner.
	assert!(!permitted);
	assert_eq!(scope.asked, ["node:node.administer"]);
}

/// A whole deletion transaction that records each step it is asked for. A
/// step the deletions below must never reach panics.
struct Deletion {
	principal: &'static str,
	authority: Fake,
	calls: Vec<String>,
}
impl Deletion {
	fn new(principal: &'static str, allowed: &[&str]) -> Self {
		Self {
			principal,
			authority: fake(allowed),
			calls: vec![],
		}
	}
	fn call(&mut self, name: &str) {
		self.calls.push(name.to_owned());
	}
}
#[async_trait]
impl DeletionAuthority for Deletion {
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.call(&format!("decide:{}:{action}", resource.kind));
		self.authority.decide(resource, action).await
	}
	async fn node_resource(&mut self, thread: Uuid) -> Result<Resource> {
		self.authority.node_resource(thread).await
	}
}
#[async_trait]
impl crate::ports::capabilities::cleanup::CleanupScope for Deletion {
	fn principal(&self) -> &str {
		self.principal
	}
	fn credential(&self) -> Uuid {
		unreachable!("credential")
	}
	fn bundle(&self) -> &aidash_domain::policy::PolicyBundle {
		unreachable!("bundle")
	}
	fn limits(&self) -> Result<crate::ports::capabilities::cleanup::Limits> {
		unreachable!("limits")
	}
	fn set_context(&mut self, _value: Value) {
		unreachable!("set_context")
	}
	fn resource(&self, _kind: &str, _id: Uuid, _attributes: Value) -> Resource {
		unreachable!("resource")
	}
	async fn serialize(&mut self) -> Result<()> {
		self.call("serialize");
		Ok(())
	}
	async fn thread(&mut self, _id: Uuid) -> Result<Resource> {
		self.call("thread");
		Ok(thread("alice"))
	}
	async fn thread_deleted(&mut self, _id: Uuid) -> Result<bool> {
		unreachable!("thread_deleted")
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.call(&format!("require:{}:{action}", resource.kind));
		Ok(())
	}
	async fn sources(&mut self, _thread: Uuid, _constraints: &Value) -> Result<()> {
		unreachable!("sources")
	}
	async fn reference_active(&mut self, _id: Uuid) -> Result<bool> {
		unreachable!("reference_active")
	}
	async fn message(&mut self, _thread: Uuid, _id: Uuid) -> Result<()> {
		unreachable!("message")
	}
	async fn visible_thread(&mut self, _thread: Uuid) -> Result<()> {
		self.call("visible_thread");
		Ok(())
	}
	async fn load_area(&mut self, _id: Uuid) -> Result<Option<Area>> {
		unreachable!("load_area")
	}
	async fn inventory(&mut self, _cursor: Option<Uuid>) -> Result<Vec<Area>> {
		unreachable!("inventory")
	}
	async fn inventory_recovery(
		&mut self,
		_area: &Area,
	) -> Result<Option<aidash_domain::capabilities::records::Record>> {
		unreachable!("inventory_recovery")
	}
	async fn cleanup_operation(&mut self, _area: &Area) -> Result<Option<Uuid>> {
		unreachable!("cleanup_operation")
	}
	async fn recoverable(
		&mut self,
		_area: &Area,
	) -> Result<Option<aidash_domain::capabilities::records::Record>> {
		unreachable!("recoverable")
	}
	async fn record_area(&mut self, _id: Uuid) -> Result<Option<Option<Uuid>>> {
		unreachable!("record_area")
	}
	async fn occupied(&mut self, _area: &Area) -> Result<Option<Uuid>> {
		unreachable!("occupied")
	}
	async fn reattach_working(&mut self, _area: &Area, _file: Uuid) -> Result<()> {
		unreachable!("reattach_working")
	}
	async fn load_record(
		&mut self,
		_id: Uuid,
		_kind: &str,
	) -> Result<aidash_domain::capabilities::records::Record> {
		unreachable!("load_record")
	}
	async fn create(
		&mut self,
		record: Creation,
	) -> Result<aidash_domain::capabilities::records::Record> {
		self.call(&format!("create:{}", record.kind));
		Ok(aidash_domain::capabilities::records::Record {
			id: record.id,
			tenant: "acme".into(),
			owner: self.principal.into(),
			area_id: record.area,
			kind: record.kind.into(),
			state: record.state.into(),
			revision: 1,
			data: record.data,
			expires_at: record.expires,
		})
	}
	async fn update(
		&mut self,
		_record: &mut aidash_domain::capabilities::records::Record,
	) -> Result<()> {
		unreachable!("update")
	}
	async fn cached(&mut self, _key: Uuid, _digest: &str) -> Result<Option<Value>> {
		self.call("cached");
		Ok(None)
	}
	async fn cache(&mut self, _key: Uuid, _digest: &str, _result: &Value) -> Result<()> {
		self.call("cache");
		Ok(())
	}
	async fn copy_owned(
		&mut self,
		_area: Uuid,
		_kind: &str,
		_file: &aidash_domain::capabilities::operations::MountedFile,
	) -> Result<aidash_domain::capabilities::operations::MountedFile> {
		unreachable!("copy_owned")
	}
	async fn verified(
		&mut self,
		_file: &aidash_domain::capabilities::operations::MountedFile,
	) -> Result<()> {
		unreachable!("verified")
	}
	async fn publish(&mut self, _area: &mut Area) -> Result<()> {
		unreachable!("publish")
	}
	async fn persist(&mut self, _area: &Area, _restoration: bool) -> Result<()> {
		unreachable!("persist")
	}
	async fn cancel_runs(&mut self, _area: &Area) -> Result<()> {
		unreachable!("cancel_runs")
	}
	async fn revoke_grants(&mut self, _area: &Area) -> Result<()> {
		unreachable!("revoke_grants")
	}
}
#[async_trait]
impl ThreadScope for Deletion {
	async fn lock_for_deletion(&mut self, _thread: Uuid) -> Result<()> {
		self.call("lock_for_deletion");
		Ok(())
	}
	async fn mark_deleted(&mut self, _thread: Uuid) -> Result<()> {
		self.call("mark_deleted");
		Ok(())
	}
	async fn delete_attachments(&mut self, _thread: Uuid) -> Result<()> {
		self.call("delete_attachments");
		Ok(())
	}
	async fn stop_schedules(&mut self, _thread: Uuid) -> Result<()> {
		self.call("stop_schedules");
		Ok(())
	}
	async fn stop_work(&mut self, _thread: Uuid) -> Result<()> {
		self.call("stop_work");
		Ok(())
	}
	async fn event(&mut self, _thread: Uuid, kind: &str, _data: Value) -> Result<()> {
		self.call(&format!("event:{kind}"));
		Ok(())
	}
}
fn request() -> DeleteThread {
	DeleteThread {
		idempotency_key: Uuid::from_u128(7),
	}
}

#[rstest]
#[case::reader("bob", &["thread:thread.read"])]
#[case::reader_holding_delete("bob", &["thread:thread.read", "thread:thread.delete"])]
#[case::owner_without_delete("alice", &["thread:thread.read"])]
#[tokio::test]
async fn a_refused_deletion_touches_nothing_of_the_thread(
	#[case] principal: &'static str,
	#[case] allowed: &[&str],
) {
	// Arrange: Alice owns the Thread; the caller may read it.
	let mut scope = Deletion::new(principal, allowed);
	// Act
	let result = delete(&mut scope, Uuid::nil(), request()).await;
	// Assert: refused after the authority checks, before any lock, Area,
	// attachment, Duty, tombstone or event.
	assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
	let side_effects = [
		"lock_for_deletion",
		"visible_thread",
		"delete_attachments",
		"stop_schedules",
		"mark_deleted",
		"stop_work",
		"create:thread_tombstone",
		"cache",
		"event:thread.deleted",
	];
	assert!(
		scope
			.calls
			.iter()
			.all(|call| !side_effects.contains(&call.as_str())),
		"{:?}",
		scope.calls
	);
	assert_eq!(scope.calls.last().unwrap(), "decide:node:node.administer");
}

#[rstest]
#[case::owner("alice", &["thread:thread.read", "thread:thread.delete"])]
#[case::node_administrator("bob", &["thread:thread.read", "node:node.administer"])]
#[tokio::test]
async fn a_permitted_deletion_marks_the_thread_before_its_tombstone(
	#[case] principal: &'static str,
	#[case] allowed: &[&str],
) {
	// Arrange
	let mut scope = Deletion::new(principal, allowed);
	// Act
	let result = delete(&mut scope, Uuid::nil(), request()).await.unwrap();
	// Assert: authority first, then the locked Thread is marked deleted in
	// the same transaction as its Duties, attachments and tombstone.
	assert_eq!(result["state"], "deleted");
	let position = |name: &str| scope.calls.iter().position(|call| call == name).unwrap();
	assert_eq!(
		scope.calls[..4],
		[
			"serialize",
			"cached",
			"thread",
			"require:thread:thread.read"
		]
	);
	assert!(position("lock_for_deletion") > position("require:thread:thread.read"));
	assert!(position("delete_attachments") > position("lock_for_deletion"));
	assert!(position("stop_schedules") > position("lock_for_deletion"));
	assert!(position("mark_deleted") > position("stop_schedules"));
	// Everyone's work in the Thread stops with it, not only the deleter's.
	assert!(position("stop_work") > position("mark_deleted"));
	assert!(position("create:thread_tombstone") > position("stop_work"));
	assert!(position("event:thread.deleted") > position("create:thread_tombstone"));
}

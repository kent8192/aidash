use super::*;
use crate::ports::capabilities::retention::RetentionLimits;
use aidash_domain::{
	capabilities::{operations::FileScope, retention::RetentionHold},
	policy::Resource,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

fn mounted(id: u128, path: &str, size: u64, media_type: &str) -> MountedFile {
	MountedFile {
		file_id: Uuid::from_u128(id),
		path: path.into(),
		digest: "a".repeat(64),
		size,
		media_type: media_type.into(),
		scope: FileScope::Working,
		provenance: json!({"kind":"patch"}),
	}
}

fn area(manifest: Vec<MountedFile>) -> Area {
	Area {
		id: Uuid::from_u128(1),
		tenant: "acme".into(),
		home_node: "node".into(),
		workspace_id: Uuid::from_u128(2),
		thread_id: Uuid::from_u128(3),
		agent_id: "research".into(),
		owner: "alice".into(),
		generation: 1,
		revision: 7,
		epoch: 1,
		state: "active".into(),
		manifest: json!(manifest),
		constraints: json!([]),
		next_sequence: 0,
	}
}

struct Fake {
	area: Area,
	holds: Vec<RetentionHold>,
	used: u64,
	reclaimable: u64,
	working_bytes: u64,
	retained_bytes: u64,
	inserts: usize,
	required: Vec<String>,
}

impl Fake {
	fn new(manifest: Vec<MountedFile>) -> Self {
		Self {
			area: area(manifest),
			holds: vec![],
			used: 0,
			reclaimable: 0,
			working_bytes: 1 << 20,
			retained_bytes: 1 << 20,
			inserts: 0,
			required: vec![],
		}
	}
}

#[async_trait]
impl RetentionScope for Fake {
	fn tenant(&self) -> &str {
		"acme"
	}
	fn principal(&self) -> &str {
		"alice"
	}
	fn limits(&self) -> Result<RetentionLimits> {
		Ok(RetentionLimits {
			working_bytes: self.working_bytes,
			retained_bytes: self.retained_bytes,
		})
	}
	fn set_context(&mut self, _: Value) {}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		Resource {
			tenant: self.tenant().into(),
			kind: kind.into(),
			id: id.to_string(),
			attributes,
		}
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		Ok(self.resource("workspace", id, json!({})))
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.required.push(format!("{}:{action}", resource.kind));
		Ok(())
	}
	async fn area(&mut self, id: Uuid) -> Result<Option<Area>> {
		Ok((self.area.id == id).then(|| self.area.clone()))
	}
	async fn holds(&mut self, _: Uuid) -> Result<Vec<RetentionHold>> {
		Ok(self.holds.clone())
	}
	async fn used_bytes(&mut self) -> Result<u64> {
		Ok(self.used)
	}
	async fn reclaimable_bytes(&mut self) -> Result<u64> {
		Ok(self.reclaimable)
	}
	async fn insert(&mut self, hold: &RetentionHold) -> Result<()> {
		self.inserts += 1;
		self.holds.push(hold.clone());
		Ok(())
	}
	async fn release(&mut self, holder: &Holder) -> Result<u64> {
		let before = self.holds.len();
		self.holds.retain(|hold| !hold.holder.same(holder));
		Ok((before - self.holds.len()) as u64)
	}
	async fn manifest_of(&mut self, _: Uuid, file: Uuid) -> Result<Option<Vec<RetainedFile>>> {
		Ok(self
			.holds
			.iter()
			.find(|hold| hold.covers(file))
			.map(|hold| hold.files.clone()))
	}
}

fn message(id: u128) -> Holder {
	Holder::Message {
		message_id: Uuid::from_u128(id),
	}
}

fn code(error: Error) -> String {
	match error {
		Error::Conflict(message) | Error::NotFound(message) | Error::Invalid(message) => message,
		other => other.to_string(),
	}
}

#[fixture]
fn plain() -> Fake {
	Fake::new(vec![
		mounted(10, "report.md", 100, "text/markdown"),
		mounted(11, "notes.txt", 50, "text/plain"),
	])
}

#[rstest]
#[tokio::test]
async fn retaining_a_plain_version_keeps_only_that_version(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	let hold = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	assert_eq!(hold.revision, 7);
	assert_eq!(hold.files.len(), 1);
	assert_eq!(hold.files[0].path, "report.md");
	assert_eq!(scope.inserts, 1);
	assert!(
		scope
			.required
			.contains(&"workspace:workspace.read".to_string())
	);
	assert!(
		scope
			.required
			.contains(&"working_area:file.read".to_string())
	);
}

#[rstest]
#[tokio::test]
async fn retaining_an_html_version_keeps_its_whole_revision(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	scope.area.manifest = json!(vec![
		mounted(10, "index.html", 100, "text/html"),
		mounted(11, "assets/app.js", 50, "text/javascript"),
	]);
	let hold = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	assert_eq!(hold.files.len(), 2);
	assert!(hold.covers(Uuid::from_u128(11)));
}

#[rstest]
#[tokio::test]
async fn citing_the_same_version_twice_is_idempotent(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	let first = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	let second = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	assert_eq!(first, second);
	assert_eq!(scope.inserts, 1);
}

#[rstest]
#[tokio::test]
async fn a_second_holder_can_cite_a_version_that_left_the_manifest(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	// A later publication replaced the version; the first hold still keeps it.
	scope.area.manifest = json!(vec![mounted(12, "report.md", 120, "text/markdown")]);
	scope.area.revision = 8;
	let second = retain(&mut scope, area, Uuid::from_u128(10), message(6))
		.await
		.unwrap();
	assert_eq!(second.revision, 7);
	assert_eq!(
		second.files,
		vec![RetainedFile::from(&mounted(
			10,
			"report.md",
			100,
			"text/markdown"
		))]
	);
}

#[rstest]
#[tokio::test]
async fn citing_a_reclaimed_version_is_refused(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	let error = retain(&mut scope, area, Uuid::from_u128(99), message(5))
		.await
		.unwrap_err();
	assert!(code(error).contains("no longer retained"));
	assert_eq!(scope.inserts, 0);
}

#[rstest]
#[tokio::test]
async fn a_working_area_over_its_budget_refuses_the_hold(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	scope.working_bytes = 120;
	let error = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap_err();
	assert!(code(error).starts_with("QUOTA_EXCEEDED"));
	assert_eq!(scope.inserts, 0);
}

#[rstest]
#[tokio::test]
async fn retained_bytes_count_against_the_working_area_budget(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	// A held version that left the manifest costs 100 bytes beyond the 150-byte
	// manifest, so a 200-byte budget refuses another hold.
	scope.area.manifest = json!(vec![mounted(12, "report.md", 150, "text/markdown")]);
	scope.holds = vec![RetentionHold {
		tenant: "acme".into(),
		area_id: scope.area.id,
		revision: 6,
		holder: message(4),
		files: vec![RetainedFile::from(&mounted(
			10,
			"report.md",
			100,
			"text/markdown",
		))],
	}];
	scope.working_bytes = 200;
	let error = retain(&mut scope, area, Uuid::from_u128(12), message(5))
		.await
		.unwrap_err();
	assert!(code(error).starts_with("QUOTA_EXCEEDED"));
}

#[rstest]
#[tokio::test]
async fn the_tenant_ceiling_discounts_reclaimable_bytes(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	scope.used = 101;
	scope.reclaimable = 0;
	scope.retained_bytes = 100;
	let error = retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap_err();
	assert!(code(error).starts_with("QUOTA_EXCEEDED"));

	scope.reclaimable = 1;
	retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
}

#[rstest]
#[tokio::test]
async fn releasing_a_holder_deletes_its_holds(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	retain(&mut scope, area, Uuid::from_u128(11), message(5))
		.await
		.unwrap();
	assert_eq!(scope.holds.len(), 2);
	assert_eq!(release(&mut scope, &message(5)).await.unwrap(), 2);
	assert!(scope.holds.is_empty());
}

#[rstest]
#[tokio::test]
async fn the_retained_manifest_returns_the_hold_of_the_version(#[from(plain)] mut scope: Fake) {
	let area = scope.area.id;
	scope.area.manifest = json!(vec![
		mounted(10, "index.html", 100, "text/html"),
		mounted(11, "assets/app.js", 50, "text/javascript"),
	]);
	retain(&mut scope, area, Uuid::from_u128(10), message(5))
		.await
		.unwrap();
	let manifest = retained_manifest(&mut scope, area, Uuid::from_u128(10))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(manifest.len(), 2);
	assert!(
		retained_manifest(&mut scope, area, Uuid::from_u128(11))
			.await
			.unwrap()
			.unwrap()
			.iter()
			.any(|file| file.file_id == Uuid::from_u128(10))
	);
	assert!(
		retained_manifest(&mut scope, area, Uuid::from_u128(12))
			.await
			.unwrap()
			.is_none()
	);
}

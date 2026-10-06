use super::*;
use crate::{
	capabilities::{
		operations::{FileScope, MountedFile},
		sharing::Recipient,
	},
	registry::EntityRef,
};
use chrono::Duration;
fn description() -> Description {
	let files = vec![MountedFile {
		file_id: Uuid::new_v4(),
		path: "report.txt".into(),
		digest: "digest".into(),
		size: 5,
		media_type: "text/plain".into(),
		scope: FileScope::Working,
		provenance: json!({}),
	}];
	Description {
		protocol: "file-transfer/1".into(),
		transfer_id: Uuid::new_v4(),
		source_node: "source".into(),
		target: Recipient {
			node_id: "recipient".into(),
			agent_id: "agent".into(),
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
		files,
		expires_at: Utc::now() + Duration::hours(1),
	}
}
fn receipt(d: &Description) -> Value {
	json!({"state":"committed","transfer_id":d.transfer_id,"input_digest":d.input_digest,"manifest_digest":d.manifest_digest,"receipt":{"node_id":d.target.node_id,"files":d.files.iter().map(|f|json!({"digest":f.digest,"size":f.size,"path":format!("{}/{}",d.transfer_id,f.path)})).collect::<Vec<_>>()}})
}
#[rstest::rstest]
fn complete_durable_receipt_is_accepted() {
	let d = description();
	assert!(receipt_matches(&d, &receipt(&d)));
}
#[rstest::rstest]
#[case("/state",json!("staging"))]
#[case("/transfer_id",json!(Uuid::new_v4()))]
#[case("/input_digest",json!("other"))]
#[case("/manifest_digest",json!("other"))]
#[case("/receipt/node_id",json!("other"))]
#[case("/receipt/files/0/digest",json!("other"))]
#[case("/receipt/files/0/size",json!(6))]
#[case("/receipt/files/0/path",json!("report.txt"))]
fn another_identity_or_effect_is_not_acknowledged(#[case] pointer: &str, #[case] value: Value) {
	let d = description();
	let mut r = receipt(&d);
	*r.pointer_mut(pointer).unwrap() = value;
	assert!(!receipt_matches(&d, &r));
}
#[rstest::rstest]
fn missing_or_extra_files_do_not_complete_delivery() {
	let d = description();
	let mut r = receipt(&d);
	r["receipt"]["files"] = json!([]);
	assert!(!receipt_matches(&d, &r));
	r["receipt"]["files"] = json!([{}, {}]);
	assert!(!receipt_matches(&d, &r));
}

use super::*;
use aidash_domain::federation::tool_lists::{HostPattern, ToolList, ToolPattern};

fn request() -> FederationToolRequest {
	FederationToolRequest {
		tools: ToolList::new([ToolPattern::new("file_*").unwrap()]).unwrap(),
		hosts: vec![HostPattern::new("*.example.com").unwrap()],
	}
}

#[test]
fn another_tenant_receiver_gets_one_detail_with_the_request() {
	let request = request();
	let details = authorization_details(Some(&request));
	assert_eq!(details.len(), 1);
	assert_eq!(details[0].request().unwrap(), request);
}

#[test]
fn a_receiver_without_registration_gets_no_detail() {
	assert!(authorization_details(None).is_empty());
}

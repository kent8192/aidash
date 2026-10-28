//! The Pod's session credential and the page credential are distinct and
//! unforgeable by a Pod, and only the Pod's may attach a Tenant Secret.
use super::*;

const KEY: &[u8] = b"fixture-runner-credential";

fn expires() -> DateTime<Utc> {
	DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

fn signature(domain: &[u8], id: Uuid) -> Vec<u8> {
	keyed_mac(KEY, domain, id, expires())
		.finalize()
		.into_bytes()
		.to_vec()
}

fn service(secret: Option<&str>) -> aidash_domain::egress::ExternalService {
	aidash_domain::egress::ExternalService {
		base_url: "https://api.example.test/".into(),
		methods: vec!["GET".into()],
		secret: secret.map(str::to_owned),
		authentication: Default::default(),
		limits: None,
	}
}

#[test]
fn a_page_signature_never_verifies_as_a_pod_session_and_the_other_way_round() {
	let id = Uuid::now_v7();
	let pod = signature(SESSION_DOMAIN, id);
	let page = signature(PAGE_DOMAIN, id);
	assert_ne!(pod, page, "the kinds use different domains");
	assert!(
		keyed_mac(KEY, SESSION_DOMAIN, id, expires())
			.verify_slice(&page)
			.is_err(),
		"a page token cannot be replayed as a Pod session"
	);
	assert!(
		keyed_mac(KEY, PAGE_DOMAIN, id, expires())
			.verify_slice(&pod)
			.is_err(),
		"a Pod session cannot be replayed as a page token"
	);
	assert!(
		keyed_mac(KEY, SESSION_DOMAIN, id, expires())
			.verify_slice(&pod)
			.is_ok()
	);
}

#[test]
fn the_pod_may_attach_a_tenant_secret_and_a_page_never_may() {
	let secured = service(Some("image-key"));
	assert_eq!(
		credential_name(SessionKind::Pod, &secured),
		Some("image-key")
	);
	assert_eq!(
		credential_name(SessionKind::Page, &secured),
		None,
		"a page any viewer can open must never spend a Tenant Secret"
	);
	let open = service(None);
	assert_eq!(credential_name(SessionKind::Pod, &open), None);
	assert_eq!(credential_name(SessionKind::Page, &open), None);
}

#[test]
fn the_request_record_names_the_admitting_path() {
	assert_eq!(SessionKind::Pod.via(), "pod");
	assert_eq!(SessionKind::Page.via(), "page");
	assert_eq!(SessionKind::Pod.prefix(), "session:");
	assert_eq!(SessionKind::Page.prefix(), "page:");
}

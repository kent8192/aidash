use super::*;
use rstest::rstest;

const REDIRECT: &str = "http://127.0.0.1:43123/auth/callback";
const OWNER: Uuid = Uuid::from_u128(0xa);
const OTHER: Uuid = Uuid::from_u128(0xb);

fn connection(state: State) -> PlanConnection {
	let now = Utc::now();
	PlanConnection {
		id: Uuid::now_v7(),
		tenant: "brave-otter-k3m9q2xa".into(),
		subject: "alice".into(),
		identity_id: OWNER,
		provider: Provider::Chatgpt,
		client_id: "oaiapp_fixture".into(),
		account: "user-fixture".into(),
		secret_resource: "aidash-store/plan-connections/fixture".into(),
		pinned_version: Some("aidash-store/plan-connections/fixture/versions/1".into()),
		state,
		access_expires_at: now + Duration::hours(1),
		earliest_refresh_at: None,
		refresh_expires_at: now + refresh_lifetime(),
		connected_at: now,
		refreshed_at: None,
		revision: 1,
	}
}

#[rstest]
#[case(
	429,
	Some("subscription_sharing_usage_limit_exceeded"),
	Disposition::Fail(Failure::UsageLimit)
)]
#[case(
	503,
	Some("subscription_sharing_usage_unavailable"),
	Disposition::Transient
)]
#[case(
	503,
	Some("subscription_sharing_user_unavailable"),
	Disposition::Transient
)]
#[case(
	403,
	Some("subscription_sharing_user_not_eligible"),
	Disposition::MarkIneligible
)]
#[case(
	401,
	Some("subscription_sharing_invalid_user"),
	Disposition::RefreshThenReconnect
)]
#[case(
	403,
	Some("subscription_sharing_route_not_supported"),
	Disposition::Fail(Failure::Misconfigured)
)]
#[case(
	403,
	Some("chatpass_v2_scope_not_authorized"),
	Disposition::Fail(Failure::Misconfigured)
)]
#[case(
	403,
	Some("chatpass_v2_invalid_authorization_context"),
	Disposition::Fail(Failure::Misconfigured)
)]
// Pre-stream failures may carry only `{"detail": ...}`: the status decides.
#[case(401, None, Disposition::RefreshThenReconnect)]
#[case(403, None, Disposition::Fail(Failure::Restricted))]
#[case(503, None, Disposition::Transient)]
// A 429 without the usage-limit code is an ordinary rate limit, retried.
#[case(429, None, Disposition::Transient)]
#[case(418, Some("unknown_code"), Disposition::Fail(Failure::Misconfigured))]
fn plan_errors_follow_openais_recovery_table(
	#[case] status: u16,
	#[case] code: Option<&str>,
	#[case] expected: Disposition,
) {
	assert_eq!(classify(status, code, None), expected);
}

#[rstest]
#[case(Some("model"), Unsupported::Model)]
#[case(Some("tools[0].type"), Unsupported::Tools)]
#[case(Some("input[1].content"), Unsupported::Input)]
#[case(Some("reasoning.effort"), Unsupported::Reasoning)]
#[case(Some("service_tier"), Unsupported::Other)]
#[case(None, Unsupported::Other)]
fn unsupported_capabilities_name_a_fixed_class(
	#[case] param: Option<&str>,
	#[case] expected: Unsupported,
) {
	assert_eq!(
		classify(
			400,
			Some("subscription_sharing_unsupported_capability"),
			param
		),
		Disposition::Fail(Failure::Unsupported(expected))
	);
}

#[test]
fn usage_limit_message_links_to_chatgpt_usage_settings() {
	let message = Failure::UsageLimit.to_string();
	assert!(message.starts_with("usage limit reached"));
	assert!(message.contains(USAGE_SETTINGS_URL));
}

#[rstest]
#[case("invalid_grant", true)]
#[case("invalid_refresh_token", true)]
#[case("token_expired", true)]
#[case("refresh_token_expired", true)]
#[case("refresh_token_invalidated", true)]
#[case("refresh_token_reused", true)]
#[case("invalid_client", false)]
#[case("server_error", false)]
fn only_unusable_refresh_tokens_end_the_grant(#[case] code: &str, #[case] terminal: bool) {
	assert_eq!(refresh_is_terminal(Some(code)), terminal);
	assert!(!refresh_is_terminal(None));
}

#[test]
fn plan_usage_needs_the_direct_token_scope() {
	assert!(grants_plan_usage(
		"openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
	));
	assert!(!grants_plan_usage("openid profile email offline_access"));
	assert!(!grants_plan_usage("chatgpt.tokens.use.direct.extra"));
}

#[test]
fn redirect_uris_use_the_loopback_address_and_callback_path() {
	assert_eq!(redirect_uri(43123).unwrap(), REDIRECT);
	assert!(redirect_uri(0).is_err());
	assert_eq!(
		redirect_uri(PASTE_PORT).unwrap(),
		"http://127.0.0.1:1457/auth/callback"
	);
	assert_ne!(PASTE_PORT, 1455, "Codex CLI listens on 1455");
}

#[test]
fn host_ids_are_uuid_urns_of_random_uuids() {
	let id = Uuid::new_v4();
	assert_eq!(host_id(id).unwrap(), format!("urn:uuid:{id}"));
	assert!(host_id(Uuid::now_v7()).is_err());
}

#[test]
fn callbacks_carry_code_state_and_the_issued_client() {
	let callback = parse_callback(
		"http://127.0.0.1:43123/auth/callback?code=abc&state=xyz&client_id=oaiapp_123",
		REDIRECT,
	)
	.unwrap();
	assert_eq!(callback.state, "xyz");
	assert_eq!(
		callback.result,
		CallbackResult::Code {
			code: "abc".into(),
			client_id: Some("oaiapp_123".into())
		}
	);
	// Surrounding whitespace from a paste is ignored.
	let reauthorized = parse_callback(
		" http://127.0.0.1:43123/auth/callback?state=s&code=c \n",
		REDIRECT,
	)
	.unwrap();
	assert_eq!(
		reauthorized.result,
		CallbackResult::Code {
			code: "c".into(),
			client_id: None
		}
	);
}

#[test]
fn declined_and_failed_consents_never_reach_an_exchange() {
	let denied = parse_callback(
		"http://127.0.0.1:43123/auth/callback?error=access_denied&state=s",
		REDIRECT,
	)
	.unwrap();
	assert_eq!(denied.result, CallbackResult::Denied);
	let failed = parse_callback(
		"http://127.0.0.1:43123/auth/callback?error=server_error&state=s&code=c",
		REDIRECT,
	)
	.unwrap();
	assert_eq!(failed.result, CallbackResult::Failed);
}

#[rstest]
#[case("http://localhost:43123/auth/callback?code=c&state=s")]
#[case("http://127.0.0.1:43124/auth/callback?code=c&state=s")]
#[case("http://127.0.0.1/auth/callback?code=c&state=s")]
#[case("https://127.0.0.1:43123/auth/callback?code=c&state=s")]
#[case("http://127.0.0.1:43123/callback?code=c&state=s")]
#[case("http://127.0.0.1:43123/auth/callback?code=c&state=s#fragment")]
#[case("http://user@127.0.0.1:43123/auth/callback?code=c&state=s")]
#[case("http://127.0.0.1:43123/auth/callback?code=c")]
#[case("http://127.0.0.1:43123/auth/callback?state=s")]
#[case("http://127.0.0.1:43123/auth/callback?code=c&state=s&state=t")]
#[case("http://127.0.0.1:43123/auth/callback?code=c&state=s&client_id=bad%20id")]
#[case("https://aidash.example/auth/callback?code=c&state=s")]
#[case("not a url")]
#[case("")]
fn other_urls_are_refused_before_their_parameters_are_read(#[case] url: &str) {
	assert!(parse_callback(url, REDIRECT).is_err());
}

#[test]
fn oversized_callbacks_are_refused() {
	let url = format!(
		"http://127.0.0.1:43123/auth/callback?state=s&code={}",
		"a".repeat(MAX_CALLBACK_BYTES)
	);
	assert!(parse_callback(&url, REDIRECT).is_err());
}

#[test]
fn reauthorization_keeps_the_saved_client() {
	assert_eq!(issued_client(None, Some("oaiapp_1")).unwrap(), "oaiapp_1");
	// A first registration must return an issued client.
	assert!(issued_client(None, None).is_err());
	assert!(issued_client(None, Some(DYNAMIC_CLIENT)).is_err());
	assert_eq!(issued_client(Some("oaiapp_1"), None).unwrap(), "oaiapp_1");
	assert_eq!(
		issued_client(Some("oaiapp_1"), Some("oaiapp_1")).unwrap(),
		"oaiapp_1"
	);
	assert!(issued_client(Some("oaiapp_1"), Some("oaiapp_2")).is_err());
}

#[test]
fn only_active_connections_with_live_refresh_tokens_serve_calls() {
	let now = Utc::now();
	let active = connection(State::Active);
	assert_eq!(
		active.require_usable(now).unwrap(),
		"aidash-store/plan-connections/fixture/versions/1"
	);
	let mut idle = connection(State::Active);
	idle.refresh_expires_at = now - Duration::seconds(1);
	assert_eq!(idle.require_usable(now), Err(Failure::NeedsReconnection));
	let mut unpinned = connection(State::Active);
	unpinned.pinned_version = None;
	assert_eq!(
		unpinned.require_usable(now),
		Err(Failure::NeedsReconnection)
	);
	assert_eq!(
		connection(State::NeedsReconnection).require_usable(now),
		Err(Failure::NeedsReconnection)
	);
	assert_eq!(
		connection(State::Ineligible).require_usable(now),
		Err(Failure::Ineligible)
	);
}

#[test]
fn calls_refresh_tokens_shortly_before_they_expire() {
	let now = Utc::now();
	let mut value = connection(State::Active);
	assert!(!value.needs_refresh(now));
	value.access_expires_at = now + Duration::seconds(REFRESH_MARGIN_SECONDS);
	assert!(value.needs_refresh(now));
	value.access_expires_at = now - Duration::seconds(1);
	assert!(value.needs_refresh(now));
}

/// OpenAI's `earliest_refresh_at` defers a refresh while the access token
/// still works; an expired token is refreshed regardless.
#[test]
fn refreshes_wait_for_the_earliest_refresh_time() {
	let now = Utc::now();
	let mut value = connection(State::Active);
	value.access_expires_at = now + Duration::seconds(REFRESH_MARGIN_SECONDS);
	value.earliest_refresh_at = Some(now + Duration::seconds(30));
	assert!(!value.may_refresh(now));
	assert!(!value.needs_refresh(now));
	value.earliest_refresh_at = Some(now);
	assert!(value.may_refresh(now));
	assert!(value.needs_refresh(now));
	value.earliest_refresh_at = Some(now + Duration::seconds(30));
	value.access_expires_at = now;
	assert!(value.needs_refresh(now));
}

#[test]
fn revisions_detect_stale_writers() {
	let value = connection(State::Active);
	assert!(value.check_revision(1).is_ok());
	assert!(value.check_revision(0).is_err());
}

fn mapped(id: Uuid, disabled: bool) -> MappedIdentity {
	MappedIdentity { id, disabled }
}

/// A connection serves only while the identity that connected it is the one
/// active identity mapped to its subject (#315).
#[test]
fn connections_serve_only_the_identity_that_connected_them() {
	let value = connection(State::Active);
	assert!(value.serves(&[mapped(OWNER, false)]));
	// A disabled identity mapped alongside it changes nothing.
	assert!(value.serves(&[mapped(OWNER, false), mapped(OTHER, true)]));
	// Identity Disablement pauses the connection.
	assert!(!value.serves(&[mapped(OWNER, true)]));
	// A shared subject has no sole identity.
	assert!(!value.serves(&[mapped(OWNER, false), mapped(OTHER, false)]));
	// Another identity restored while the owner is disabled is the only
	// active one, but never inherits the owner's plan.
	assert!(!value.serves(&[mapped(OWNER, true), mapped(OTHER, false)]));
	assert!(!value.serves(&[]));
	assert_eq!(
		sole_identity(&[mapped(OWNER, true), mapped(OTHER, false)]),
		Some(OTHER)
	);
	assert_eq!(sole_identity(&[mapped(OWNER, true)]), None);
}

#[test]
fn a_disabled_owner_still_holds_its_mapping() {
	let value = connection(State::Active);
	assert!(value.owner_mapped(&[mapped(OWNER, true)]));
	assert!(!value.owner_mapped(&[mapped(OTHER, false)]));
	assert!(!value.owner_mapped(&[]));
}

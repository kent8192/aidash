//! A person's own Approval Mode and Approval Rules through the production
//! router and PostgreSQL (#411, ADR 0076). Only the owner's dashboard or desktop
//! session writes them; Tenant Administrators with `approval.read` read them.
//! How a Run uses them is tested in `execution_authorization`.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
use aidash_domain::identity::tenancy::{MEMBERS, initial_policy};
use aidash_domain::policy::{Subject, SubjectKind};
use aidash_server::{
	authorization::{Authorization as Policies, policy::PolicyBundle},
	config::OidcConfig,
	federation::Federation,
};
use futures_util::FutureExt;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
use reinhardt::test::TestResponse;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:8080";
const TENANT: &str = "alpha";
const PREFERENCES: &str = "/api/tenants/alpha/approval-preferences/mine";
const RULES: &str = "/api/tenants/alpha/approval-rules/mine";
const DECIDER: &str = "/api/tenants/alpha/approval-decider";
/// The built-in approval Decider every Tenant Registry holds.
const BUILT_IN: &str = "aidash.approval-decider";
/// The tool call a member's rule names.
const TOOL: &str = "tool:crm#invoke";

async fn execute(f: &Federation, query: String) {
	sqlx::query(&query)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}
async fn insert_identity(f: &Federation, subject: &str) -> Uuid {
	let identity = Uuid::new_v4();
	execute(
		f,
		Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::val(identity))
					.expr(Expr::val("https://accounts.google.com"))
					.expr(Expr::val(subject))
					.expr(Expr::cust("clock_timestamp()"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.await;
	identity
}
async fn insert_session(f: &Federation, identity: Uuid, token: &str, csrf: &str) {
	execute(
		f,
		Query::insert()
			.into_table(Alias::new("dashboard_sessions"))
			.columns(
				[
					"id",
					"identity_id",
					"token_hash",
					"csrf_hash",
					"created_at",
					"last_activity_at",
					"expires_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::val(Uuid::new_v4()))
					.expr(Expr::val(identity))
					.expr(Expr::val(Sha256::digest(token.as_bytes()).to_vec()))
					.expr(Expr::val(Sha256::digest(csrf.as_bytes()).to_vec()))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()"))
					.expr(Expr::cust("clock_timestamp()+interval '12 hours'"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.await;
}

fn user(group: &str) -> Subject {
	Subject {
		kind: SubjectKind::User,
		groups: [group.to_owned()].into(),
		roles: Default::default(),
		attributes: json!({}),
		enabled: true,
		delegated_by: None,
	}
}

/// Three people with a browser session each: the Tenant's `owner`, an
/// administrator, and two members, `member` and `other`.
#[rstest::fixture]
fn approval_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.oidc = Some(OidcConfig {
			issuer: "https://accounts.google.com".into(),
			client_id: "fixture".into(),
			client_secret: "server-only-fixture".into(),
			public_origin: ORIGIN.into(),
			session_absolute_seconds: 43_200,
			session_idle_seconds: 1_800,
		});
		for (subject, session, csrf) in [
			("approval-owner", "browser-a", "csrf-a"),
			("approval-member", "browser-b", "csrf-b"),
			("approval-other", "browser-c", "csrf-c"),
		] {
			let identity = insert_identity(f, subject).await;
			insert_session(f, identity, session, csrf).await;
		}
		common::tenant_root(f, TENANT).await;
		let mut bundle: PolicyBundle = initial_policy(TENANT, true).unwrap();
		for member in ["member", "other"] {
			bundle.subjects.insert(member.into(), user(MEMBERS));
		}
		Policies {
			pool: f.store.pool.clone(),
		}
		.replace(TENANT, 0, bundle, "deployment_administrator")
		.await
		.unwrap();
		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn approval_application(
	#[from(approval_runtime)] _runtime: common::RuntimeFuture,
	#[from(common::direct_application)]
	#[with(aidash_server::http::Settings { auth_burst: 10000, ..Default::default() }, aidash_server::sse::Service::new(Default::default()), std::sync::Arc::new(|r| r), _runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	application
}

struct Fixture {
	f: Federation,
	app: common::TestApplication,
	url: String,
	schema: String,
}
impl Fixture {
	fn new(application: common::ApplicationFixture) -> Self {
		let (f, url, schema) = application.runtime.parts();
		Self {
			f,
			app: application.application,
			url,
			schema,
		}
	}
	async fn cleanup(self) {
		common::cleanup(self.f, &self.url, &self.schema).await;
	}
}

/// The three browsers, by what they act as.
#[derive(Clone, Copy)]
enum Person {
	Owner,
	Member,
	Other,
}
impl Person {
	fn session(self) -> (&'static str, &'static str) {
		match self {
			Self::Owner => ("browser-a", "csrf-a"),
			Self::Member => ("browser-b", "csrf-b"),
			Self::Other => ("browser-c", "csrf-c"),
		}
	}
	fn subject(self) -> &'static str {
		match self {
			Self::Owner => "owner",
			Self::Member => "member",
			Self::Other => "other",
		}
	}
}

async fn send(
	fixture: &Fixture,
	person: Person,
	context: &str,
	method: &str,
	path: &str,
	body: Value,
) -> TestResponse {
	let (session, csrf) = person.session();
	let cookie = format!("aidash-session={session}");
	let body = if body.is_null() {
		Vec::new()
	} else {
		body.to_string().into_bytes()
	};
	common::http_response(
		&fixture.app,
		method,
		path,
		&[
			("content-type", "application/json"),
			("cookie", &cookie),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf),
			("x-aidash-context", context),
		],
		&body,
	)
	.await
}
fn json_body(response: &TestResponse) -> Value {
	serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// Maps the person's session to their subject through the normal registration
/// and Deployment Administrator approval, and returns their dashboard context.
async fn map(fixture: &Fixture, person: Person) -> String {
	let (session, csrf) = person.session();
	let registration = common::http_response(
		&fixture.app,
		"POST",
		"/auth/registration",
		&[
			("cookie", &format!("aidash-session={session}")),
			("origin", ORIGIN),
			("x-aidash-csrf", csrf),
		],
		&[],
	)
	.await;
	let registration = json_body(&registration);
	let (status, mapping) = common::request(
		&fixture.app,
		&fixture.f.config.api_token,
		"POST",
		&format!(
			"/api/dashboard/registrations/{}/approve",
			registration["id"].as_str().unwrap()
		),
		json!({"tenant": TENANT, "subject": person.subject()}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	format!("mapping:{TENANT}:{}", mapping["id"].as_str().unwrap())
}

/// Everyone mapped, as the tests need them.
struct People {
	owner: String,
	member: String,
	other: String,
}
async fn people(fixture: &Fixture) -> People {
	People {
		owner: map(fixture, Person::Owner).await,
		member: map(fixture, Person::Member).await,
		other: map(fixture, Person::Other).await,
	}
}

/// Changes the Tenant's policy the way a Tenant Administrator would.
async fn amend(f: &Federation, change: impl FnOnce(&mut Value)) {
	let authorization = Policies {
		pool: f.store.pool.clone(),
	};
	let current = authorization.snapshot(TENANT).await.unwrap();
	let mut bundle = serde_json::to_value(&current.bundle).unwrap();
	change(&mut bundle);
	authorization
		.replace(
			TENANT,
			current.revision,
			serde_json::from_value(bundle).unwrap(),
			"deployment_administrator",
		)
		.await
		.expect("the Tenant policy is amended");
}
fn policy(id: &str, effect: &str, subject: Value, action: &str, kind: &str) -> Value {
	json!({
		"id": id,
		"effect": effect,
		"subjects": subject,
		"actions": [action],
		"resources": {"kinds": [kind]}
	})
}
/// Lets `member` use Bypass: the Tenant's policy names it.
async fn allow_bypass(f: &Federation) {
	amend(f, |bundle| {
		bundle["policies"].as_array_mut().unwrap().push(policy(
			"member-bypass",
			"allow",
			json!({"ids": ["member"]}),
			"approval.mode.bypass",
			"approval_mode",
		));
	})
	.await;
}

fn allowed(body: &Value, mode: &str) -> bool {
	body["modes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|choice| choice["mode"] == mode)
		.unwrap_or_else(|| panic!("{mode} is offered: {body}"))["allowed"]
		.as_bool()
		.unwrap()
}

/// Why a mode is unavailable or not yet in effect, or `null`.
fn reason(body: &Value, mode: &str) -> Value {
	body["modes"]
		.as_array()
		.unwrap()
		.iter()
		.find(|choice| choice["mode"] == mode)
		.unwrap_or_else(|| panic!("{mode} is offered: {body}"))["reason"]
		.clone()
}

/// Gives the Tenant's Catalog Approval to version 1.0.0 of `id`.
async fn approve(f: &Federation, id: &str) {
	Policies {
		pool: f.store.pool.clone(),
	}
	.set_catalog(
		TENANT,
		&aidash_domain::registry::EntityRef {
			id: id.into(),
			version: "1.0.0".into(),
		},
		0,
		true,
		"deployment_administrator",
	)
	.await
	.expect("the Decider is approved");
}

/// Registers an entry of the Tenant's Registry: a copy of the built-in approval
/// Decider under `id`, or a compaction Decider when `hook` says so.
async fn register_decider(f: &Federation, id: &str, hook: &str) {
	let mut entry = aidash_application::registry::system::approval_decider().unwrap();
	entry.id = id.into();
	entry.tags.clear();
	if hook == "compaction" {
		entry.config = json!({
			"hook": "compaction", "answer_type": "noul", "description": "Keep needed history",
			"provider_credential": "openrouter",
			"routes": [{"provider_contract": "typesafe.jev/1", "model": "typesafe/jev-1.14", "threshold": "3fe0000000000000"}],
			"builder": "aidash.compaction/2", "option_source": "history_event/1",
			"rule": "compaction.keep/1", "mode": "enforce", "max_calls_per_run": 20
		});
	}
	// A Decider that names a Provider Credential registers only where the Node
	// keeps Provider Credentials.
	f.registry
		.clone()
		.with_provider_credentials(true)
		.register(
			&aidash_domain::tenancy::TenantName::new(TENANT).unwrap(),
			entry,
		)
		.await
		.expect("the Decider is registered");
}

/// Stores `id` as the Tenant's approval Decider, whatever the Registry holds.
async fn name_decider(f: &Federation, id: &str) {
	execute(
		f,
		Query::insert()
			.into_table(Alias::new("identity_tenant_approval_settings"))
			.columns(
				[
					"tenant",
					"decider_id",
					"decider_version",
					"revision",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::val(TENANT))
					.expr(Expr::val(id))
					.expr(Expr::val("1.0.0"))
					.expr(Expr::val(1_i64))
					.expr(Expr::cust("clock_timestamp()"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_person_reads_and_saves_their_own_preferences(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;

	// Act + Assert: nothing is saved yet, so Manual applies and Bypass is out of reach.
	let fresh = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		PREFERENCES,
		Value::Null,
	)
	.await;
	let body = json_body(&fresh);
	assert_eq!(fresh.status(), 200, "{body}");
	assert_eq!(fresh.headers()["cache-control"], "no-store");
	assert_eq!(body["default_mode"], "manual");
	assert_eq!(body["revision"], 0);
	assert_eq!(body["answer_deadline_seconds"], Value::Null);
	assert_eq!(body["bypass_accepted_at"], Value::Null);
	assert!(allowed(&body, "manual") && allowed(&body, "dont_ask"));
	assert!(!allowed(&body, "bypass"));
	assert_eq!(reason(&body, "bypass"), "policy");
	assert_eq!(reason(&body, "manual"), Value::Null);

	let saved = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		PREFERENCES,
		json!({"expected_revision": 0, "default_mode": "dont_ask", "answer_deadline_seconds": 120}),
	)
	.await;
	let body = json_body(&saved);
	assert_eq!(saved.status(), 200, "{body}");
	assert_eq!(body["default_mode"], "dont_ask");
	assert_eq!(body["answer_deadline_seconds"], 120);
	assert_eq!(body["revision"], 1);

	// A write from a stale reading conflicts.
	let stale = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		PREFERENCES,
		json!({"expected_revision": 0, "default_mode": "auto"}),
	)
	.await;
	assert_eq!(stale.status(), 409, "{}", json_body(&stale));
	let next = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		PREFERENCES,
		json!({"expected_revision": 1, "default_mode": "auto"}),
	)
	.await;
	let body = json_body(&next);
	assert_eq!(next.status(), 200, "{body}");
	assert_eq!(
		(
			&body["default_mode"],
			&body["revision"],
			&body["answer_deadline_seconds"]
		),
		(&json!("auto"), &json!(2), &Value::Null)
	);

	// The Answer Deadline may be as short as a minute and as long as 30 days.
	for (revision, seconds) in [(2, 60), (3, 30 * 24 * 60 * 60)] {
		let kept = send(
			&fixture,
			Person::Member,
			&people.member,
			"PUT",
			PREFERENCES,
			json!({"expected_revision": revision, "default_mode": "manual", "answer_deadline_seconds": seconds}),
		)
		.await;
		let body = json_body(&kept);
		assert_eq!(kept.status(), 200, "{body}");
		assert_eq!(body["answer_deadline_seconds"], seconds);
	}

	// Bounds and shapes are refused before anything is stored.
	for refused in [
		json!({"expected_revision": 4, "default_mode": "manual", "answer_deadline_seconds": 0}),
		json!({"expected_revision": 4, "default_mode": "manual", "answer_deadline_seconds": 59}),
		json!({"expected_revision": 4, "default_mode": "manual", "answer_deadline_seconds": 30 * 24 * 60 * 60 + 1}),
		json!({"expected_revision": 4, "default_mode": "manual", "unexpected": true}),
		json!({"expected_revision": -1, "default_mode": "manual"}),
	] {
		let response = send(
			&fixture,
			Person::Member,
			&people.member,
			"PUT",
			PREFERENCES,
			refused.clone(),
		)
		.await;
		assert!(
			matches!(response.status().as_u16(), 400 | 422),
			"{refused}: {}",
			response.status()
		);
	}
	let unchanged = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		PREFERENCES,
		Value::Null,
	)
	.await;
	let body = json_body(&unchanged);
	assert_eq!(
		(&body["answer_deadline_seconds"], &body["revision"]),
		(&json!(30 * 24 * 60 * 60), &json!(4))
	);

	// Another person's preferences are their own.
	let other = send(
		&fixture,
		Person::Other,
		&people.other,
		"GET",
		PREFERENCES,
		Value::Null,
	)
	.await;
	let body = json_body(&other);
	assert_eq!(
		(&body["default_mode"], &body["revision"]),
		(&json!("manual"), &json!(0))
	);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bypass_needs_the_policy_and_the_persons_acceptance_once(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let put = async |mode: &str, revision: i64, accept: bool| {
		send(
			&fixture,
			Person::Member,
			&people.member,
			"PUT",
			PREFERENCES,
			json!({"expected_revision": revision, "default_mode": mode, "accept_bypass": accept}),
		)
		.await
	};

	// Act + Assert: a member cannot select Bypass until the Tenant's policy names it.
	assert_eq!(put("bypass", 0, true).await.status(), 403);
	allow_bypass(&fixture.f).await;
	let offered = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		PREFERENCES,
		Value::Null,
	)
	.await;
	assert!(allowed(&json_body(&offered), "bypass"));

	// Acceptance is explicit, and recorded at the first use.
	let unaccepted = put("bypass", 0, false).await;
	assert_eq!(unaccepted.status(), 400, "{}", json_body(&unaccepted));
	let accepted = put("bypass", 0, true).await;
	let body = json_body(&accepted);
	assert_eq!(accepted.status(), 200, "{body}");
	assert_eq!(body["default_mode"], "bypass");
	let at = body["bypass_accepted_at"]
		.as_str()
		.expect("acceptance is recorded")
		.to_owned();

	// Leaving Bypass keeps the acceptance, so returning asks nothing again.
	assert_eq!(put("manual", 1, false).await.status(), 200);
	let again = put("bypass", 2, false).await;
	let body = json_body(&again);
	assert_eq!(again.status(), 200, "{body}");
	assert_eq!(body["bypass_accepted_at"], at);

	// A policy that stops naming Bypass leaves the mode stored and out of reach.
	amend(&fixture.f, |bundle| {
		bundle["policies"]
			.as_array_mut()
			.unwrap()
			.retain(|policy| policy["id"] != "member-bypass");
	})
	.await;
	let lowered = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		PREFERENCES,
		Value::Null,
	)
	.await;
	let body = json_body(&lowered);
	assert_eq!(body["default_mode"], "bypass");
	assert!(!allowed(&body, "bypass"));
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn rules_are_the_owners_and_report_their_ceiling(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let post = async |person: Person, context: &str, body: Value| {
		send(&fixture, person, context, "POST", RULES, body).await
	};

	// Act: a member allows a tool, which members approve themselves.
	let tool = post(
		Person::Member,
		&people.member,
		json!({"kind": "tool_call", "target": {"exact": TOOL}, "effect": "allow"}),
	)
	.await;
	let tool_body = json_body(&tool);
	assert_eq!(tool.status(), 201, "{tool_body}");
	assert_eq!(tool_body["effective"], true);
	assert_eq!(tool_body["kind"], "tool_call");
	assert_eq!(tool_body["target"], json!({"exact": TOOL}));

	// Assert: the same rule stored twice conflicts, a misshapen target is refused,
	// and a rule on what members cannot approve is kept but not effective.
	let duplicate = post(
		Person::Member,
		&people.member,
		json!({"kind": "tool_call", "target": {"exact": TOOL}, "effect": "allow"}),
	)
	.await;
	assert_eq!(duplicate.status(), 409, "{}", json_body(&duplicate));
	let misshapen = post(
		Person::Member,
		&people.member,
		json!({"kind": "outbound", "target": {"exact": "example.com"}, "effect": "allow"}),
	)
	.await;
	assert_eq!(misshapen.status(), 400, "{}", json_body(&misshapen));
	let outbound = post(
		Person::Member,
		&people.member,
		json!({"kind": "outbound", "target": {"exact": "https://example.com"}, "effect": "allow"}),
	)
	.await;
	let outbound_body = json_body(&outbound);
	assert_eq!(outbound.status(), 201, "{outbound_body}");
	assert_eq!(
		outbound_body["effective"], false,
		"members cannot approve outbound reads"
	);
	let every = post(
		Person::Member,
		&people.member,
		json!({"kind": "tool_call", "target": "every", "effect": "ask"}),
	)
	.await;
	assert_eq!(every.status(), 201, "{}", json_body(&every));

	// The owner reads exactly their own rules, in the order they made them.
	let list = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		RULES,
		Value::Null,
	)
	.await;
	let listed = json_body(&list);
	assert_eq!(list.status(), 200, "{listed}");
	assert_eq!(list.headers()["cache-control"], "no-store");
	let rules = listed["rules"].as_array().unwrap();
	assert_eq!(rules.len(), 3, "{listed}");
	assert_eq!(rules[0]["id"], tool_body["id"]);
	let theirs = send(
		&fixture,
		Person::Other,
		&people.other,
		"GET",
		RULES,
		Value::Null,
	)
	.await;
	assert_eq!(json_body(&theirs)["rules"], json!([]));

	// Nobody deletes another person's rule, and the owner deletes their own once.
	let path = format!("{RULES}/{}", tool_body["id"].as_str().unwrap());
	let stranger = send(
		&fixture,
		Person::Other,
		&people.other,
		"DELETE",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(stranger.status(), 404);
	let deleted = send(
		&fixture,
		Person::Member,
		&people.member,
		"DELETE",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(deleted.status(), 204);
	let again = send(
		&fixture,
		Person::Member,
		&people.member,
		"DELETE",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(again.status(), 404);
	let remaining = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		RULES,
		Value::Null,
	)
	.await;
	assert_eq!(json_body(&remaining)["rules"].as_array().unwrap().len(), 2);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn subject_credentials_and_deployment_administrators_never_reach_them(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange: a Subject Credential for the member who owns the settings.
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let administrator = fixture.f.config.api_token.clone();
	let (status, issued) = common::request(
		&fixture.app,
		&administrator,
		"POST",
		&format!("/api/authorization/{TENANT}/credentials"),
		json!({"subject": "member"}),
	)
	.await;
	assert_eq!(status, 200, "{issued}");
	let credential = issued["token"].as_str().unwrap().to_owned();
	let thread = format!(
		"/api/workspaces/{}/threads/{}/approval-mode",
		Uuid::new_v4(),
		Uuid::new_v4()
	);

	// Act + Assert: bearers are refused on every route.
	for token in [&credential, &administrator] {
		for (method, path, body) in [
			("GET", PREFERENCES.to_owned(), Value::Null),
			(
				"PUT",
				PREFERENCES.to_owned(),
				json!({"expected_revision": 0, "default_mode": "manual"}),
			),
			("GET", RULES.to_owned(), Value::Null),
			(
				"POST",
				RULES.to_owned(),
				json!({"kind": "tool_call", "target": "every", "effect": "ask"}),
			),
			("DELETE", format!("{RULES}/{}", Uuid::new_v4()), Value::Null),
			(
				"GET",
				format!("/api/tenants/{TENANT}/subjects/member/approval-settings"),
				Value::Null,
			),
			("GET", thread.clone(), Value::Null),
			("PUT", thread.clone(), json!({"mode": null})),
		] {
			let (status, body) = common::request(&fixture.app, token, method, &path, body).await;
			assert_eq!(status, 403, "{method} {path}: {body}");
		}
	}
	// Another Tenant's path is never the person's.
	let elsewhere = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		"/api/tenants/beta/approval-preferences/mine",
		Value::Null,
	)
	.await;
	assert_eq!(elsewhere.status(), 403);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn administrators_read_a_members_settings_with_approval_read_and_cannot_write_them(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange: the member saves preferences and two rules.
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let saved = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		PREFERENCES,
		json!({"expected_revision": 0, "default_mode": "dont_ask", "answer_deadline_seconds": 60}),
	)
	.await;
	assert_eq!(saved.status(), 200, "{}", json_body(&saved));
	for rule in [
		json!({"kind": "tool_call", "target": {"exact": TOOL}, "effect": "allow"}),
		json!({"kind": "outbound", "target": "every", "effect": "deny"}),
	] {
		let response = send(
			&fixture,
			Person::Member,
			&people.member,
			"POST",
			RULES,
			rule,
		)
		.await;
		assert_eq!(response.status(), 201, "{}", json_body(&response));
	}
	let path = format!("/api/tenants/{TENANT}/subjects/member/approval-settings");

	// Act
	let read = send(
		&fixture,
		Person::Owner,
		&people.owner,
		"GET",
		&path,
		Value::Null,
	)
	.await;

	// Assert: the administrator sees the member's settings, each rule with its ceiling.
	let body = json_body(&read);
	assert_eq!(read.status(), 200, "{body}");
	assert_eq!(body["subject"], "member");
	assert_eq!(body["default_mode"], "dont_ask");
	assert_eq!(body["answer_deadline_seconds"], 60);
	assert_eq!(body["revision"], 1);
	let rules = body["rules"].as_array().unwrap();
	assert_eq!(rules.len(), 2, "{body}");
	assert!(rules.iter().all(|rule| rule["effective"] == true), "{body}");
	assert_eq!(rules[0]["effect"], "allow");
	assert_eq!(rules[0]["target"], json!({"exact": TOOL}));

	// Members hold no `approval.read`, not even for themselves or for an administrator.
	for subject in ["member", "owner", "other"] {
		let denied = send(
			&fixture,
			Person::Member,
			&people.member,
			"GET",
			&format!("/api/tenants/{TENANT}/subjects/{subject}/approval-settings"),
			Value::Null,
		)
		.await;
		assert_eq!(denied.status(), 403, "{subject}");
	}
	// Administrators change none of it, and a subject the policy lacks is not found.
	let write = send(
		&fixture,
		Person::Owner,
		&people.owner,
		"PUT",
		&path,
		json!({"expected_revision": 1, "default_mode": "manual"}),
	)
	.await;
	assert!(
		matches!(write.status().as_u16(), 404 | 405),
		"{}",
		write.status()
	);
	let missing = send(
		&fixture,
		Person::Owner,
		&people.owner,
		"GET",
		&format!("/api/tenants/{TENANT}/subjects/nobody/approval-settings"),
		Value::Null,
	)
	.await;
	assert_eq!(missing.status(), 404);

	// A lowered ceiling keeps the rules and reports them as not effective.
	amend(&fixture.f, |bundle| {
		bundle["policies"].as_array_mut().unwrap().push(policy(
			"no-advance",
			"deny",
			json!({"kinds": ["user"]}),
			"approval.advance",
			"*",
		));
	})
	.await;
	let effective = |body: &Value, effect: &str| {
		body["rules"]
			.as_array()
			.unwrap()
			.iter()
			.find(|rule| rule["effect"] == effect)
			.unwrap_or_else(|| panic!("a {effect} rule is listed: {body}"))["effective"]
			.clone()
	};
	let lowered = json_body(
		&send(
			&fixture,
			Person::Owner,
			&people.owner,
			"GET",
			&path,
			Value::Null,
		)
		.await,
	);
	assert_eq!(lowered["rules"].as_array().unwrap().len(), 2, "{lowered}");
	assert_eq!(effective(&lowered, "allow"), false, "{lowered}");
	assert_eq!(
		effective(&lowered, "deny"),
		true,
		"a deny relaxes nothing: {lowered}"
	);
	let own = json_body(
		&send(
			&fixture,
			Person::Member,
			&people.member,
			"GET",
			RULES,
			Value::Null,
		)
		.await,
	);
	assert_eq!(effective(&own, "allow"), false);

	// A Tenant that withholds `approval.read` from its administrators hides the settings.
	amend(&fixture.f, |bundle| {
		bundle["policies"].as_array_mut().unwrap().push(policy(
			"no-approval-read",
			"deny",
			json!({"ids": ["owner"]}),
			"approval.read",
			"subject",
		));
	})
	.await;
	let hidden = send(
		&fixture,
		Person::Owner,
		&people.owner,
		"GET",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(hidden.status(), 403);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_thread_mode_is_the_callers_own_and_needs_read_access_to_the_thread(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange: the member starts a Workspace and a Thread.
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let created = send(
		&fixture,
		Person::Member,
		&people.member,
		"POST",
		"/api/workspaces",
		json!({"title": "Approvals", "goal": "Thread modes"}),
	)
	.await;
	let workspace = json_body(&created)["id"].as_str().unwrap().to_owned();
	assert_eq!(created.status(), 200);
	let message = send(
		&fixture,
		Person::Member,
		&people.member,
		"POST",
		&format!("/api/workspaces/{workspace}/thread-messages"),
		json!({"content": "Which mode?", "idempotency_key": Uuid::new_v4()}),
	)
	.await;
	let message_body = json_body(&message);
	assert_eq!(message.status(), 200, "{message_body}");
	let thread = send(
		&fixture,
		Person::Member,
		&people.member,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id": message_body["message"]["id"]}),
	)
	.await;
	let thread_body = json_body(&thread);
	assert_eq!(thread.status(), 200, "{thread_body}");
	let path = format!(
		"/api/workspaces/{workspace}/threads/{}/approval-mode",
		thread_body["id"].as_str().unwrap()
	);

	// Act + Assert: nothing is set, so the default applies.
	let fresh = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		&path,
		Value::Null,
	)
	.await;
	let body = json_body(&fresh);
	assert_eq!(fresh.status(), 200, "{body}");
	assert_eq!(body["mode"], Value::Null);
	assert_eq!(body["default_mode"], "manual");
	assert!(allowed(&body, "dont_ask") && !allowed(&body, "bypass"));

	let set = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		&path,
		json!({"mode": "dont_ask"}),
	)
	.await;
	assert_eq!(set.status(), 200, "{}", json_body(&set));
	assert_eq!(json_body(&set)["mode"], "dont_ask");
	let read = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(json_body(&read)["mode"], "dont_ask");

	// The same ceiling and acceptance as the default.
	let beyond = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		&path,
		json!({"mode": "bypass"}),
	)
	.await;
	assert_eq!(beyond.status(), 403);
	allow_bypass(&fixture.f).await;
	let unaccepted = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		&path,
		json!({"mode": "bypass"}),
	)
	.await;
	assert_eq!(unaccepted.status(), 400, "{}", json_body(&unaccepted));
	let accepted = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		PREFERENCES,
		json!({"expected_revision": 0, "default_mode": "bypass", "accept_bypass": true}),
	)
	.await;
	assert_eq!(accepted.status(), 200, "{}", json_body(&accepted));
	let bypass = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		&path,
		json!({"mode": "bypass"}),
	)
	.await;
	assert_eq!(bypass.status(), 200, "{}", json_body(&bypass));

	// `null` clears it, and the default applies again.
	let cleared = send(
		&fixture,
		Person::Member,
		&people.member,
		"PUT",
		&path,
		json!({"mode": null}),
	)
	.await;
	assert_eq!(cleared.status(), 200, "{}", json_body(&cleared));
	assert_eq!(json_body(&cleared)["mode"], Value::Null);
	let unset = json_body(
		&send(
			&fixture,
			Person::Member,
			&people.member,
			"GET",
			&path,
			Value::Null,
		)
		.await,
	);
	assert_eq!(unset["mode"], Value::Null);
	assert_eq!(unset["default_mode"], "bypass");

	// Another reader's mode for the Thread is their own.
	let theirs = send(
		&fixture,
		Person::Other,
		&people.other,
		"PUT",
		&path,
		json!({"mode": "dont_ask"}),
	)
	.await;
	assert_eq!(theirs.status(), 200, "{}", json_body(&theirs));
	let mine = json_body(
		&send(
			&fixture,
			Person::Member,
			&people.member,
			"GET",
			&path,
			Value::Null,
		)
		.await,
	);
	assert_eq!(mine["mode"], Value::Null);

	// Someone who cannot read the Workspace neither reads nor sets a mode for its Thread,
	// and a Thread that is not there is not found.
	amend(&fixture.f, |bundle| {
		bundle["policies"].as_array_mut().unwrap().push(policy(
			"no-workspace-read",
			"deny",
			json!({"ids": ["other"]}),
			"workspace.read",
			"*",
		));
	})
	.await;
	let stranger = send(
		&fixture,
		Person::Other,
		&people.other,
		"GET",
		&path,
		Value::Null,
	)
	.await;
	assert_eq!(stranger.status(), 403);
	let stranger = send(
		&fixture,
		Person::Other,
		&people.other,
		"PUT",
		&path,
		json!({"mode": "manual"}),
	)
	.await;
	assert_eq!(stranger.status(), 403);
	let missing = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		&format!(
			"/api/workspaces/{workspace}/threads/{}/approval-mode",
			Uuid::new_v4()
		),
		Value::Null,
	)
	.await;
	assert_eq!(missing.status(), 404);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn auto_is_offered_only_while_the_tenants_approval_decider_has_a_catalog_approval(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let read = async || {
		let response = send(
			&fixture,
			Person::Member,
			&people.member,
			"GET",
			PREFERENCES,
			Value::Null,
		)
		.await;
		assert_eq!(response.status(), 200);
		json_body(&response)
	};

	// Act + Assert: the policy names Auto, yet nobody approved the Decider.
	let body = read().await;
	assert!(!allowed(&body, "auto"), "{body}");
	assert_eq!(reason(&body, "auto"), "decider_unapproved");

	approve(&fixture.f, BUILT_IN).await;
	let body = read().await;
	assert!(allowed(&body, "auto"), "{body}");
	assert_eq!(reason(&body, "auto"), Value::Null);

	// A Decider the Registry does not hold leaves Auto without one.
	name_decider(&fixture.f, "gone").await;
	let body = read().await;
	assert!(!allowed(&body, "auto"), "{body}");
	assert_eq!(reason(&body, "auto"), "no_decider");

	// Bypass is the member's to choose once the policy names it, and takes
	// effect after the person accepts its risk.
	allow_bypass(&fixture.f).await;
	let body = read().await;
	assert!(allowed(&body, "bypass"));
	assert_eq!(reason(&body, "bypass"), "not_accepted");
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_member_reads_the_tenants_approval_decider(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	let read = async |person: Person, context: &str| {
		send(&fixture, person, context, "GET", DECIDER, Value::Null).await
	};

	// Act
	let fresh = read(Person::Member, &people.member).await;

	// Assert: the built-in Decider judges until the Tenant names another.
	let body = json_body(&fresh);
	assert_eq!(fresh.status(), 200, "{body}");
	assert_eq!(fresh.headers()["cache-control"], "no-store");
	assert_eq!(body["decider"], json!({"id": BUILT_IN, "version": "1.0.0"}));
	assert_eq!(body["built_in"], true);
	assert_eq!(body["registered"], true, "{body}");
	assert_eq!(body["approved"], false);
	assert_eq!(body["revision"], 0);
	approve(&fixture.f, BUILT_IN).await;
	let approved = json_body(&read(Person::Other, &people.other).await);
	assert_eq!(approved["approved"], true);
	fixture.cleanup().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_tenant_update_changes_the_decider_and_it_must_be_an_approval_decider(
	#[future(awt)]
	#[from(approval_application)]
	application: common::ApplicationFixture,
) {
	// Arrange
	let fixture = Fixture::new(application);
	let people = people(&fixture).await;
	register_decider(&fixture.f, "strict-approval", "approval").await;
	register_decider(&fixture.f, "plain-compaction", "compaction").await;
	let put = async |person: Person, context: &str, body: Value| {
		send(&fixture, person, context, "PUT", DECIDER, body).await
	};

	// Act + Assert: a member lacks `tenant.update`.
	let refused = put(
		Person::Member,
		&people.member,
		json!({"expected_revision": 0, "decider": {"id": "strict-approval", "version": "1.0.0"}}),
	)
	.await;
	assert_eq!(refused.status(), 403, "{}", json_body(&refused));

	// Only a Decider of hook approval can be named.
	for (target, why) in [
		(
			json!({"id": "plain-compaction", "version": "1.0.0"}),
			"a compaction Decider",
		),
		(
			json!({"id": "gone", "version": "1.0.0"}),
			"an unknown entry",
		),
		(
			json!({"id": "strict-approval", "version": "9.9.9"}),
			"an unknown version",
		),
	] {
		let response = put(
			Person::Owner,
			&people.owner,
			json!({"expected_revision": 0, "decider": target}),
		)
		.await;
		assert_eq!(response.status(), 400, "{why}: {}", json_body(&response));
	}
	let shape = put(
		Person::Owner,
		&people.owner,
		json!({"expected_revision": 0, "decider": {"id": "strict-approval", "version": "1.0.0"}, "unexpected": true}),
	)
	.await;
	assert!(matches!(shape.status().as_u16(), 400 | 422));

	// The owner names it; the read follows.
	let named = put(
		Person::Owner,
		&people.owner,
		json!({"expected_revision": 0, "decider": {"id": "strict-approval", "version": "1.0.0"}}),
	)
	.await;
	let body = json_body(&named);
	assert_eq!(named.status(), 200, "{body}");
	assert_eq!(
		body["decider"],
		json!({"id": "strict-approval", "version": "1.0.0"})
	);
	assert_eq!(
		(
			&body["built_in"],
			&body["registered"],
			&body["approved"],
			&body["revision"]
		),
		(&json!(false), &json!(true), &json!(false), &json!(1))
	);
	let read = send(
		&fixture,
		Person::Member,
		&people.member,
		"GET",
		DECIDER,
		Value::Null,
	)
	.await;
	assert_eq!(json_body(&read)["decider"]["id"], "strict-approval");

	// A stale write conflicts; the reset to the built-in keeps counting.
	let stale = put(
		Person::Owner,
		&people.owner,
		json!({"expected_revision": 0, "decider": null}),
	)
	.await;
	assert_eq!(stale.status(), 409, "{}", json_body(&stale));
	let reset = put(
		Person::Owner,
		&people.owner,
		json!({"expected_revision": 1, "decider": null}),
	)
	.await;
	let body = json_body(&reset);
	assert_eq!(reset.status(), 200, "{body}");
	assert_eq!(body["decider"]["id"], BUILT_IN);
	assert_eq!(
		(&body["built_in"], &body["revision"]),
		(&json!(true), &json!(2))
	);

	// The Tenant of the path is the caller's own.
	let elsewhere = send(
		&fixture,
		Person::Owner,
		&people.owner,
		"PUT",
		"/api/tenants/beta/approval-decider",
		json!({"expected_revision": 0, "decider": null}),
	)
	.await;
	assert_eq!(elsewhere.status(), 403, "{}", json_body(&elsewhere));
	fixture.cleanup().await;
}

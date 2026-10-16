//! Tenancy Rules, the Tenant creation API, assigned Tenant names, Invitations
//! and the last-administrator guard (#238), over HTTP and PostgreSQL.
use super::*;
use aidash_domain::identity::tenancy::TenancyRules;

const DEPLOYMENT_ADMINISTRATOR: &str = "deployment-administrator-execution-fixture";

/// One deployment with direct Google sign-in and the given Tenancy Rules.
struct Deployment {
	federation: aidash_server::federation::Federation,
	app: common::TestApplication,
	url: String,
	database: String,
	_runtime: common::RuntimeFixture,
}

impl Deployment {
	async fn new(runtime: common::RuntimeFuture, rules: TenancyRules) -> Self {
		let runtime = runtime.await;
		let (mut federation, url, database) = runtime.parts();
		federation.config.oidc = Some(OidcConfig {
			issuer: "https://accounts.google.com".into(),
			client_id: "aidash".into(),
			client_secret: "test-secret".into(),
			public_origin: "http://127.0.0.1:8080".into(),
			keycloak_admin_url: String::new(),
			status_client_id: String::new(),
			status_client_secret: String::new(),
			session_absolute_seconds: 43_200,
			session_idle_seconds: 1_800,
		});
		federation.config.tenancy = rules;
		let app = common::application(federation.clone()).await;
		Self {
			federation,
			app,
			url,
			database,
			_runtime: runtime,
		}
	}

	/// A signed-in External Identity with Display Attributes; its browser
	/// session and CSRF secrets are both `name`.
	async fn person(&self, name: &str) -> Person {
		let identity = Uuid::new_v4();
		let pool = self.federation.store.pool.driver();
		sqlx::query(
			"INSERT INTO dashboard_identities (id, issuer, subject, last_valid_at, verified_email, display_name, display_observed_at) \
			 VALUES ($1, 'https://accounts.google.com', $2, clock_timestamp(), $3, $2, clock_timestamp())",
		)
		.bind(identity)
		.bind(name)
		.bind(format!("{name}@example.com"))
		.execute(pool)
		.await
		.unwrap();
		sqlx::query(
			"INSERT INTO dashboard_sessions (id, token_hash, csrf_hash, identity_id, created_at, last_activity_at, expires_at) \
			 VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp(), clock_timestamp() + interval '12 hours')",
		)
		.bind(Uuid::new_v4())
		.bind(Sha256::digest(name.as_bytes()).to_vec())
		.bind(Sha256::digest(name.as_bytes()).to_vec())
		.bind(identity)
		.execute(pool)
		.await
		.unwrap();
		Person {
			name: name.into(),
			identity,
		}
	}

	async fn send(
		&self,
		method: &str,
		path: &str,
		person: Option<&Person>,
		context: Option<&str>,
		bearer: Option<&str>,
		body: Value,
	) -> (u16, Value, http::HeaderMap) {
		let mut request = self
			.app
			.api_http
			.request(
				Method::from_bytes(method.as_bytes()).unwrap(),
				&self.app.url(path),
			)
			.header("content-type", "application/json");
		if let Some(person) = person {
			request = request
				.header("cookie", format!("aidash-session={}", person.name))
				.header("origin", "http://127.0.0.1:8080")
				.header("x-aidash-csrf", person.name.as_str());
		}
		if let Some(context) = context {
			request = request.header("x-aidash-context", context);
		}
		if let Some(token) = bearer {
			request = request.header("authorization", format!("Bearer {token}"));
		}
		let response = request.json(&body).send().await.unwrap();
		let status = response.status().as_u16();
		let headers = response.headers().clone();
		let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
		(status, body, headers)
	}

	async fn as_deployment_administrator(
		&self,
		method: &str,
		path: &str,
		body: Value,
	) -> (u16, Value) {
		let (status, body, _) = self
			.send(
				method,
				path,
				None,
				None,
				Some(DEPLOYMENT_ADMINISTRATOR),
				body,
			)
			.await;
		(status, body)
	}

	async fn as_person(
		&self,
		person: &Person,
		method: &str,
		path: &str,
		context: Option<&str>,
		body: Value,
	) -> (u16, Value) {
		let (status, body, _) = self
			.send(method, path, Some(person), context, None, body)
			.await;
		(status, body)
	}

	/// The person's enabled Mapping into `tenant`, as a context selector.
	async fn context(&self, person: &Person, tenant: &str) -> String {
		let (status, session) = self
			.as_person(person, "GET", "/auth/session", None, Value::Null)
			.await;
		assert_eq!(status, 200, "{session}");
		let mapping = session["mappings"]
			.as_array()
			.unwrap()
			.iter()
			.find(|mapping| mapping["tenant"] == tenant)
			.unwrap_or_else(|| panic!("no Mapping into {tenant}: {session}"));
		format!("mapping:{}", mapping["id"].as_str().unwrap())
	}

	async fn count(&self, sql: &str) -> i64 {
		sqlx::query_scalar(sql)
			.fetch_one(self.federation.store.pool.driver())
			.await
			.unwrap()
	}

	async fn finish(self) {
		drop(self.app);
		common::cleanup(self.federation, &self.url, &self.database).await;
	}
}

struct Person {
	name: String,
	identity: Uuid,
}

fn open_rules() -> TenancyRules {
	TenancyRules {
		tenant_creation: true,
		..TenancyRules::default()
	}
}

fn assert_assigned_name(name: &str) {
	let parts: Vec<_> = name.split('-').collect();
	assert_eq!(parts.len(), 3, "{name}");
	assert_eq!(parts[2].len(), 8, "{name}");
	assert!(name.len() <= 63);
}

#[rstest::rstest]
#[tokio::test]
async fn deployment_administrators_create_tenants_under_assigned_names(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, TenancyRules::default()).await;

	let (status, created) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"  Research  "}))
		.await;

	assert_eq!(status, 201, "{created}");
	let name = created["name"].as_str().unwrap().to_owned();
	assert_assigned_name(&name);
	assert_eq!(created["label"], "Research");
	assert!(created["mapping_id"].is_null());
	let (status, policy) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{name}"), Value::Null)
		.await;
	assert_eq!(status, 200, "{policy}");
	assert_eq!(policy["revision"], 1);
	assert_eq!(
		policy["bundle"]["groups"]["administrators"]["assignable"],
		true
	);
	assert_eq!(policy["bundle"]["groups"]["members"]["assignable"], true);
	assert!(policy["bundle"]["subjects"].as_object().unwrap().is_empty());
	let (status, revisions) = d
		.as_deployment_administrator(
			"GET",
			&format!("/api/authorization/{name}/revisions"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{revisions}");
	assert_eq!(revisions[0]["actor"], "deployment_administrator");
	let (status, tenants) = d
		.as_deployment_administrator("GET", "/api/tenants", Value::Null)
		.await;
	assert_eq!(status, 200, "{tenants}");
	assert_eq!(tenants[0]["name"], name);
	assert_eq!(tenants[0]["created_by_kind"], "deployment_administrator");
	// A second Tenant never takes the first one's name.
	let (_, second) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Research"}))
		.await;
	assert_ne!(second["name"], name);
	// The list pages from the newest Tenant.
	let (status, page) = d
		.as_deployment_administrator("GET", "/api/tenants?offset=1", Value::Null)
		.await;
	assert_eq!(status, 200, "{page}");
	let total = tenants.as_array().unwrap().len();
	assert_eq!(page.as_array().unwrap().len(), total, "{page}");
	assert_eq!(page[0]["name"], tenants[0]["name"]);
	for label in ["", "   ", "line\nbreak"] {
		let (status, _) = d
			.as_deployment_administrator("POST", "/api/tenants", json!({"label":label}))
			.await;
		assert_eq!(status, 400, "{label:?}");
	}
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_policy_needs_a_root_record_so_tenants_are_never_created_implicitly(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, TenancyRules::default()).await;

	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			"/api/authorization/chosen-name",
			json!({"expected_revision":0,"bundle":{"tenant":"chosen-name","subjects":{},"policies":[]}}),
		)
		.await;

	assert_eq!(status, 404, "{body}");
	assert_eq!(d.count("SELECT count(*) FROM tenants").await, 0);
	assert_eq!(
		d.count("SELECT count(*) FROM authorization_bundles").await,
		0
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_signed_in_people_and_deployment_administrators_create_tenants(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let closed = Deployment::new(runtime.clone(), TenancyRules::default()).await;
	let alice = closed.person("alice").await;
	let (status, body) = closed
		.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":"Team"}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	let (_, session) = closed
		.as_person(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert_eq!(session["tenant_creation"], false);
	closed.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_creator_administers_the_tenant_it_created(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (_, session) = d
		.as_person(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert_eq!(session["tenant_creation"], true);

	let (status, created) = d
		.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":"Team"}),
		)
		.await;

	assert_eq!(status, 201, "{created}");
	let tenant = created["name"].as_str().unwrap().to_owned();
	assert!(created["mapping_id"].is_string());
	let context = d.context(&alice, &tenant).await;
	let (status, overview) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/administration"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{overview}");
	assert!(
		overview["actions"]
			.as_array()
			.unwrap()
			.contains(&json!("invitation.create"))
	);
	let (status, revisions) = d
		.as_deployment_administrator(
			"GET",
			&format!("/api/authorization/{tenant}/revisions"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{revisions}");
	assert_eq!(revisions[0]["actor"], format!("oidc:{}", alice.identity));
	// A Subject Credential never creates a Tenant, even with every action.
	let (status, issued) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}/credentials"),
			json!({"subject":"owner"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let token = issued["token"].as_str().unwrap();
	let (status, _, _) = d
		.send(
			"POST",
			"/api/tenants",
			None,
			None,
			Some(token),
			json!({"label":"Agentic"}),
		)
		.await;
	assert_eq!(status, 403);
	// The label is the creator's to change, under `tenant.update`.
	let (status, renamed) = d
		.as_person(
			&alice,
			"PUT",
			&format!("/api/tenants/{tenant}/label"),
			Some(&context),
			json!({"label":"Renamed team"}),
		)
		.await;
	assert_eq!(status, 200, "{renamed}");
	let (_, session) = d
		.as_person(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert!(
		session["mappings"]
			.as_array()
			.unwrap()
			.iter()
			.any(|mapping| mapping["tenant"] == tenant.as_str()
				&& mapping["label"] == "Renamed team")
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_creations_stop_exactly_at_the_total_limit(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			creation_total: 3,
			creation_per_hour: 100,
			..open_rules()
		},
	)
	.await;
	let alice = d.person("alice").await;

	let results = join_all((0..8).map(|index| {
		d.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":format!("Team {index}")}),
		)
	}))
	.await;

	let created = results.iter().filter(|(status, _)| *status == 201).count();
	assert_eq!(created, 3, "{results:?}");
	assert!(
		results
			.iter()
			.filter(|(status, _)| *status != 201)
			.all(|(status, body)| *status == 403 && body["error"] == "tenant_limit_reached"),
		"{results:?}"
	);
	// Deployment Administrators are never limited.
	let (status, _) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Ops"}))
		.await;
	assert_eq!(status, 201);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_hourly_limit_answers_when_to_retry(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			creation_per_hour: 2,
			..open_rules()
		},
	)
	.await;
	let alice = d.person("alice").await;
	for index in 0..2 {
		let (status, body) = d
			.as_person(
				&alice,
				"POST",
				"/api/tenants",
				None,
				json!({"label":format!("T{index}")}),
			)
			.await;
		assert_eq!(status, 201, "{body}");
	}

	let (status, body, headers) = d
		.send(
			"POST",
			"/api/tenants",
			Some(&alice),
			None,
			None,
			json!({"label":"T2"}),
		)
		.await;

	assert_eq!(status, 429, "{body}");
	let retry: i64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
	assert!((3_000..=3_600).contains(&retry), "{retry}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitations_are_single_use_tenant_addressed_and_approved(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let carol = d.person("carol").await;
	let (_, created) = d
		.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":"Team"}),
		)
		.await;
	let tenant = created["name"].as_str().unwrap().to_owned();
	let context = d.context(&alice, &tenant).await;
	// A Tenant's own administrator never enumerates the Deployment's Tenants.
	let (status, _) = d
		.as_person(&alice, "GET", "/api/tenants", Some(&context), Value::Null)
		.await;
	assert_eq!(status, 403);

	let (status, invitation) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({}),
		)
		.await;

	assert_eq!(status, 201, "{invitation}");
	assert_eq!(invitation["groups"], json!(["members"]));
	let secret = invitation["secret"].as_str().unwrap().to_owned();
	assert_eq!(
		invitation["link"],
		format!("http://127.0.0.1:8080/invitations/{tenant}#{secret}")
	);
	let link = json!({"tenant":tenant,"secret":secret});
	// Opening the link shows the Tenant and spends nothing.
	for _ in 0..2 {
		let (status, preview) = d
			.as_person(
				&bob,
				"POST",
				"/auth/invitations/inspect",
				None,
				link.clone(),
			)
			.await;
		assert_eq!(status, 200, "{preview}");
		assert_eq!(preview["label"], "Team");
	}
	let (status, request) = d
		.as_person(&bob, "POST", "/auth/invitations/redeem", None, link.clone())
		.await;
	assert_eq!(status, 200, "{request}");
	assert_eq!(request["status"], "pending");
	assert_eq!(request["tenant"], tenant.as_str());
	// A spent link joins nobody, whoever holds it.
	for person in [&bob, &carol] {
		let (status, body) = d
			.as_person(
				person,
				"POST",
				"/auth/invitations/redeem",
				None,
				link.clone(),
			)
			.await;
		assert_eq!(status, 404, "{body}");
		assert_eq!(body["error"], "invalid link");
	}
	// Nobody joins on redemption: an administrator approves.
	assert!(
		d.context_or_none(&bob, &tenant).await.is_none(),
		"redeeming must not map Bob"
	);
	let (status, pending) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/registrations"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending.as_array().unwrap().len(), 1, "{pending}");
	assert_eq!(pending[0]["identity"]["verified_email"], "bob@example.com");
	assert_eq!(pending[0]["proposed_groups"], json!(["members"]));
	let id = pending[0]["id"].as_str().unwrap();
	let (status, approved) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/registrations/{id}/approve"),
			Some(&context),
			json!({"subject":"bob","groups":["members"]}),
		)
		.await;
	assert_eq!(status, 200, "{approved}");
	let bob_context = d.context(&bob, &tenant).await;
	// A member uses the Tenant but does not administer it.
	let (status, _) = d
		.as_person(
			&bob,
			"PUT",
			&format!("/api/tenants/{tenant}/label"),
			Some(&bob_context),
			json!({"label":"Taken over"}),
		)
		.await;
	assert_eq!(status, 403);
	let (status, _) = d
		.as_person(
			&bob,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&bob_context),
			json!({}),
		)
		.await;
	assert_eq!(status, 403);
	// The administrator recognizes Bob, who chose this Tenant.
	let (status, mappings) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/mappings"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{mappings}");
	assert!(
		mappings
			.as_array()
			.unwrap()
			.iter()
			.any(|mapping| mapping["identity"]["verified_email"] == "bob@example.com")
	);
	// A revoked or expired link joins nobody either.
	let (_, revoked) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({"expires_in_seconds":60}),
		)
		.await;
	let (status, _) = d
		.as_person(
			&alice,
			"POST",
			&format!(
				"/api/tenants/{tenant}/invitations/{}/revoke",
				revoked["id"].as_str().unwrap()
			),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200);
	let (status, _) = d
		.as_person(
			&carol,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":tenant,"secret":revoked["secret"]}),
		)
		.await;
	assert_eq!(status, 404);
	let (_, expired) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({"expires_in_seconds":60}),
		)
		.await;
	sqlx::query("UPDATE tenant_invitations SET created_at = created_at - interval '2 minutes', expires_at = expires_at - interval '2 minutes' WHERE id = $1")
		.bind(Uuid::parse_str(expired["id"].as_str().unwrap()).unwrap())
		.execute(d.federation.store.pool.driver())
		.await
		.unwrap();
	let (status, _) = d
		.as_person(
			&carol,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":tenant,"secret":expired["secret"]}),
		)
		.await;
	assert_eq!(status, 404);
	assert_eq!(
		d.count(
			"SELECT count(*) FROM dashboard_administration_history WHERE action LIKE 'invitation.%'"
		)
		.await,
		5
	);
	d.finish().await;
}

impl Deployment {
	async fn context_or_none(&self, person: &Person, tenant: &str) -> Option<String> {
		let (_, session) = self
			.as_person(person, "GET", "/auth/session", None, Value::Null)
			.await;
		session["mappings"]
			.as_array()
			.unwrap()
			.iter()
			.find(|mapping| mapping["tenant"] == tenant)
			.map(|mapping| format!("mapping:{}", mapping["id"].as_str().unwrap()))
	}

	/// Invites `person` into `groups` of `tenant` and approves the request as
	/// the Deployment Administrator, which may create the subject in any group.
	async fn admit(&self, tenant: &str, person: &Person, subject: &str, groups: Value) -> String {
		let (status, invitation) = self
			.as_deployment_administrator(
				"POST",
				&format!("/api/tenants/{tenant}/invitations"),
				json!({"groups":groups}),
			)
			.await;
		assert_eq!(status, 201, "{invitation}");
		let (status, request) = self
			.as_person(
				person,
				"POST",
				"/auth/invitations/redeem",
				None,
				json!({"tenant":tenant,"secret":invitation["secret"]}),
			)
			.await;
		assert_eq!(status, 200, "{request}");
		let (status, approved) = self
			.as_deployment_administrator(
				"POST",
				&format!(
					"/api/dashboard/registrations/{}/approve",
					request["id"].as_str().unwrap()
				),
				json!({"tenant":tenant,"subject":subject,"groups":groups}),
			)
			.await;
		assert_eq!(status, 200, "{approved}");
		self.context(person, tenant).await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_deployment_administrator_created_tenant_gets_its_first_administrator_by_invitation(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, TenancyRules::default()).await;
	let dana = d.person("dana").await;
	let (_, created) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Customer"}))
		.await;
	let tenant = created["name"].as_str().unwrap().to_owned();

	let context = d
		.admit(&tenant, &dana, "dana", json!(["administrators"]))
		.await;

	let (status, overview) = d
		.as_person(
			&dana,
			"GET",
			&format!("/api/tenants/{tenant}/administration"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{overview}");
	assert_eq!(
		overview["actions"].as_array().unwrap().len(),
		10,
		"{overview}"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn members_cannot_remove_the_last_unconditional_administrator(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (_, created) = d
		.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":"Team"}),
		)
		.await;
	let tenant = created["name"].as_str().unwrap().to_owned();
	let bob_context = d
		.admit(&tenant, &bob, "bob", json!(["administrators"]))
		.await;
	// Bob's administration now holds only under a condition, so it never
	// counts as unconditional.
	let (_, policy) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{tenant}"), Value::Null)
		.await;
	let mut bundle = policy["bundle"].clone();
	bundle["subjects"]["bob"]["groups"] = json!([]);
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"bob-over-api","effect":"allow","subjects":{"ids":["bob"]},"actions":["*"],
		"resources":{"kinds":["*"]},
		"condition":{"op":"eq","left":{"source":"environment","path":"/transport"},
			"right":{"source":"literal","value":"api"}}
	}));
	let (status, replaced) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}"),
			json!({"expected_revision":policy["revision"],"bundle":bundle}),
		)
		.await;
	assert_eq!(status, 200, "{replaced}");
	let (_, mappings) = d
		.as_person(
			&bob,
			"GET",
			&format!("/api/tenants/{tenant}/mappings"),
			Some(&bob_context),
			Value::Null,
		)
		.await;
	let alice_mapping = mappings
		.as_array()
		.unwrap()
		.iter()
		.find(|mapping| mapping["subject"] == "owner")
		.unwrap()
		.clone();

	let (demote, _) = d
		.as_person(
			&bob,
			"PUT",
			&format!("/api/tenants/{tenant}/subjects/owner/groups"),
			Some(&bob_context),
			json!({"groups":[],"expected_policy_revision":replaced["revision"]}),
		)
		.await;
	let (disable, _) = d
		.as_person(
			&bob,
			"POST",
			&format!(
				"/api/tenants/{tenant}/mappings/{}/disable",
				alice_mapping["id"].as_str().unwrap()
			),
			Some(&bob_context),
			json!({"expected_revision":alice_mapping["revision"]}),
		)
		.await;

	assert_eq!(demote, 409);
	assert_eq!(disable, 409);
	// A Deployment Administrator's whole-policy replacement is a deliberate
	// deployment-wide action and is never blocked.
	let (_, current) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{tenant}"), Value::Null)
		.await;
	let mut orphaning = current["bundle"].clone();
	orphaning["subjects"]["owner"]["groups"] = json!([]);
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}"),
			json!({"expected_revision":current["revision"],"bundle":orphaning}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn personal_tenants_follow_the_rules_and_are_never_restored(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			personal_tenants: true,
			..TenancyRules::default()
		},
	)
	.await;
	let alice = d.person("alice").await;

	let sessions =
		join_all((0..4).map(|_| d.as_person(&alice, "GET", "/auth/session", None, Value::Null)))
			.await;

	let (status, session) = &sessions[0];
	assert_eq!(*status, 200, "{session}");
	assert_eq!(session["personal_account"], true);
	let personal = &session["mappings"][0];
	assert_eq!(personal["personal"], true);
	assert_eq!(personal["label"], "Personal");
	let tenant = personal["tenant"].as_str().unwrap().to_owned();
	assert_assigned_name(&tenant);
	assert!(!tenant.contains(&alice.identity.simple().to_string()));
	assert_eq!(
		d.count("SELECT count(*) FROM tenants WHERE personal").await,
		1
	);
	// Disabling the Personal Tenant's Mapping is a durable decision.
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!(
				"/api/dashboard/mappings/{}/disable",
				personal["id"].as_str().unwrap()
			),
			json!({"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 204, "{body}");
	let (status, session) = d
		.as_person(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert_eq!(status, 200, "{session}");
	assert!(
		session["mappings"].as_array().unwrap().is_empty(),
		"{session}"
	);
	assert_eq!(
		d.count("SELECT count(*) FROM tenants WHERE personal").await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn invitations_follow_the_rules(#[from(common::runtime)] runtime: common::RuntimeFuture) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			invitations: false,
			..TenancyRules::default()
		},
	)
	.await;
	let (_, created) = d
		.as_deployment_administrator("POST", "/api/tenants", json!({"label":"Team"}))
		.await;

	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!(
				"/api/tenants/{}/invitations",
				created["name"].as_str().unwrap()
			),
			json!({}),
		)
		.await;

	assert_eq!(status, 403, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_pending_invitation_request_keeps_the_display_attributes_its_administrators_need(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (_, created) = d
		.as_person(
			&alice,
			"POST",
			"/api/tenants",
			None,
			json!({"label":"Team"}),
		)
		.await;
	let tenant = created["name"].as_str().unwrap().to_owned();
	let context = d.context(&alice, &tenant).await;
	let (_, invitation) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({}),
		)
		.await;
	let (status, _) = d
		.as_person(
			&bob,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 200);

	// A retention pass without sign-in configuration counts no session or
	// recent sign-in: only authority or a pending request can retain.
	let mut unconfigured = d.federation.clone();
	unconfigured.config.oidc = None;
	let (stop, stopping) = tokio::sync::watch::channel(false);
	stop.send(true).unwrap();
	aidash_server::dashboard_auth::enforce_display_retention(unconfigured, stopping)
		.await
		.unwrap();

	let (_, pending) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/registrations"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(
		pending[0]["identity"]["verified_email"], "bob@example.com",
		"{pending}"
	);
	d.finish().await;
}

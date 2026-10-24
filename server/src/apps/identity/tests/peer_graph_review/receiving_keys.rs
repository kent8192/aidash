//! Receiving trust is owned by actual HTTP admission and PostgreSQL CAS. Crypto
//! details are covered by capability; this fixture owns persistence/lifecycle.
use super::*;
use aidash_application::federation::key::{DeploymentKey, Signing};
use aidash_application::{
	federation::inbound::{self, RequestDigest},
	ports::federation::inbound::{
		InboundRepository, ReceivingKeyOwner, ReceivingKeys, ViewerGrant,
	},
};
use aidash_capability::{
	InMemorySigner, TokenSigner,
	federation::{
		FederationClaims, body_digest,
		rollover::{RolloverClaims, mint_transition},
	},
};
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{
		DeploymentId, DeploymentKeyId, NodeAddress, NodeName,
		keys::{PublicDeploymentKey, RolloverSchedule, TrustedDeploymentKeys},
		registration::NodeRegistration,
	},
	tenancy::TenantName,
};
use aidash_integrations::federation::key::FileDeploymentKey;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::ExposeSecret;

fn successor() -> InMemorySigner {
	let temporary = InMemorySigner::new("temporary".into(), [9; 32]);
	InMemorySigner::new(
		DeploymentKeyId::of_public_key(&temporary.public_key().to_bytes()).to_string(),
		[9; 32],
	)
}

async fn proof(predecessor: &dyn DeploymentKey, successor: &InMemorySigner, now: u64) -> String {
	mint_transition(
		&RolloverClaims {
			deployment: SOURCE.into(),
			predecessor_kid: predecessor.key_id().to_string(),
			successor_kid: successor.kid().into(),
			successor_public_key: URL_SAFE_NO_PAD.encode(successor.public_key().to_bytes()),
			published_at: now - 90_000,
			activates_at: now - 3_600,
			retires_at: now - 3_600 + 604_800,
		},
		&Signing(predecessor),
	)
	.await
	.unwrap()
}

async fn token(
	signer: &dyn TokenSigner,
	sender: &str,
	receiver: &str,
	subject: &str,
	body: &Value,
	proof: Option<String>,
	now: u64,
	wrong_binding: bool,
) -> String {
	FederationClaims {
		iss: SOURCE.into(),
		sub: sender.into(),
		aud: receiver.into(),
		subject: subject.into(),
		iat: now,
		exp: now + 60,
		jti: Uuid::new_v4(),
		kid: signer.kid().into(),
		protocol: aidash_domain::federation::protocol::CURRENT.into(),
		method: if wrong_binding { "DELETE" } else { "POST" }.into(),
		path: "/scoped/graph".into(),
		body: body_digest(body.to_string().as_bytes()),
		transition: proof,
	}
	.mint(signer)
	.await
	.unwrap()
	.expose_secret()
	.to_owned()
}

fn graph(viewer: Value, limit: u64, cursor: Value) -> Value {
	let target = (viewer["kind"] == "deployment_administrator").then_some("acme");
	json!({"viewer":viewer,"target_tenant":target,"scope_workspace":null,"mode":"mesh","kinds":["workspace"],"relations":[],"hours":0,"limit":limit,"cursor":cursor})
}

async fn post(app: &common::TestApplication, raw: &str, body: &Value) -> (u16, Value) {
	let response = common::peering::post_as(
		&app.client(),
		"unused",
		common::peering::Bearer::Raw(raw),
		"unused",
		"/federation/v0.1/scoped/graph",
		body,
	)
	.await;
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}

/// Only time is virtual. All authority/key reads and writes delegate to the
/// restarted production PostgreSQL adapter, with no production test seam.
struct Clock<'a>(&'a Federation, u64);
#[async_trait]
impl InboundRepository for Clock<'_> {
	fn deployment(&self) -> &DeploymentId {
		self.0.deployment()
	}
	fn federation_enabled(&self) -> bool {
		self.0.federation_enabled()
	}
	fn holds_key(&self) -> bool {
		self.0.holds_key()
	}
	fn now(&self) -> u64 {
		self.1
	}
	async fn node_tenant(&self, node: &NodeName) -> aidash_application::Result<Option<TenantName>> {
		self.0.node_tenant(node).await
	}
	async fn tenant_live(&self, tenant: &TenantName) -> aidash_application::Result<bool> {
		self.0.tenant_live(tenant).await
	}
	async fn registration(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
	) -> aidash_application::Result<Option<NodeRegistration>> {
		self.0.registration(tenant, receiver, sender).await
	}
	async fn mapping(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
		subject: &str,
	) -> aidash_application::Result<Option<Mapping>> {
		self.0.mapping(tenant, receiver, sender, subject).await
	}
	async fn subject_may_use(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		subject: &str,
	) -> aidash_application::Result<bool> {
		self.0.subject_may_use(tenant, receiver, subject).await
	}
	async fn viewer_grant(
		&self,
		tenant: &TenantName,
		viewer: &DeploymentId,
		identity: Uuid,
	) -> aidash_application::Result<Option<ViewerGrant>> {
		self.0.viewer_grant(tenant, viewer, identity).await
	}
	async fn receiving_keys(
		&self,
		owner: &ReceivingKeyOwner,
	) -> aidash_application::Result<Option<ReceivingKeys>> {
		self.0.receiving_keys(owner).await
	}
	async fn advance_keys(
		&self,
		owner: &ReceivingKeyOwner,
		kr: i64,
		ar: i64,
		keys: &TrustedDeploymentKeys,
	) -> aidash_application::Result<bool> {
		self.0.advance_keys(owner, kr, ar, keys).await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn receiving_rollover_survives_discovery_outage_restart_and_cursor_continuation(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, local_token, _) = bootstrap(&f, &app, "http://localhost:1").await;
	let (subject, _, source) = viewer(&f, &app).await;
	for title in ["second", "third"] {
		let (status, body) = request(
			&app,
			&local_token,
			"POST",
			"/api/workspaces",
			json!({"title":title,"goal":"cursor"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let identity = Uuid::new_v4();
	let (status, body) = request(&app, &f.config.api_token, "POST", "/api/authorization/acme/peer-graph-grants", json!({"source_deployment":SOURCE,"source_identity":identity,"endpoint":source.remote.endpoint,"key_id":source.remote.key_id(),"enabled":true,"expected_revision":0})).await;
	assert_eq!(status, 200, "{body}");
	let predecessor = FileDeploymentKey::load(&source.remote.key.path).unwrap();
	let sender = NodeAddress::parse(&source.address()).unwrap();
	let receiver = source.receiver.clone();
	let endpoint = source.remote.endpoint.clone();
	drop(source); // Actual well-known endpoint is now unavailable.
	assert!(
		reqwest::Client::new()
			.get(format!("{endpoint}/.well-known/aidash"))
			.send()
			.await
			.is_err()
	);
	let next = successor();
	let now = chrono::Utc::now().timestamp() as u64;
	let proof = proof(&predecessor, &next, now).await;
	let owner = ReceivingKeyOwner::Registration {
		tenant: TenantName::new("acme").unwrap(),
		node: receiver.node.clone(),
		source_node: sender.clone(),
	};
	let grant_owner = ReceivingKeyOwner::ViewerGrant {
		tenant: TenantName::new("acme").unwrap(),
		viewer: DeploymentId::parse(SOURCE).unwrap(),
		identity,
	};
	let subject_first = graph(subject.clone(), 2, Value::Null);
	let pred = token(
		&Signing(&predecessor),
		&sender.to_string(),
		&receiver.to_string(),
		"source-subject",
		&subject_first,
		None,
		now,
		false,
	)
	.await;
	let (status, first) = post(&app, &pred, &subject_first).await;
	assert_eq!(status, 200, "{first}");
	assert!(first["next_cursor"].is_string(), "{first}");
	let body = graph(subject, 2, first["next_cursor"].clone());

	let before = f.receiving_keys(&owner).await.unwrap().unwrap();

	let bad = token(
		&next,
		&sender.to_string(),
		&receiver.to_string(),
		"source-subject",
		&body,
		Some(proof.clone()),
		now,
		true,
	)
	.await;
	assert_eq!(post(&app, &bad, &body).await.0, 401);
	assert_eq!(f.receiving_keys(&owner).await.unwrap().unwrap(), before);
	let raw = token(
		&next,
		&sender.to_string(),
		&receiver.to_string(),
		"source-subject",
		&body,
		Some(proof.clone()),
		now,
		false,
	)
	.await;
	let (status, page) = post(&app, &raw, &body).await;
	assert_eq!(status, 200, "{page}");
	let learned = f.receiving_keys(&owner).await.unwrap().unwrap();
	assert_eq!(learned.keys.current.key_id.as_str(), next.kid());
	assert_eq!(learned.keys_revision, before.keys_revision + 1);
	assert_eq!(
		f.registration(&TenantName::new("acme").unwrap(), &receiver.node, &sender)
			.await
			.unwrap()
			.unwrap()
			.revision,
		1
	);
	assert_eq!(
		f.receiving_keys(&grant_owner)
			.await
			.unwrap()
			.unwrap()
			.keys_revision,
		1
	);
	assert_eq!(post(&app, &raw, &body).await.0, 200); // duplicate does not advance again
	assert_eq!(f.receiving_keys(&owner).await.unwrap().unwrap(), learned);
	let history: i64 =
		sqlx::query_scalar("SELECT count(*) FROM node_registration_history WHERE tenant='acme'")
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(history, 1);
	let first_body = graph(
		json!({"kind":"deployment_administrator","id":identity}),
		2,
		Value::Null,
	);
	let pred = token(
		&Signing(&predecessor),
		SOURCE,
		f.config.deployment.as_str(),
		&format!("oidc:{identity}"),
		&first_body,
		None,
		now,
		false,
	)
	.await;
	let (status, first) = post(&app, &pred, &first_body).await;
	assert_eq!(status, 200, "{first}");
	assert!(first["next_cursor"].is_string(), "{first}");
	let continued_body = graph(
		json!({"kind":"deployment_administrator","id":identity}),
		2,
		first["next_cursor"].clone(),
	);
	let raw = token(
		&next,
		SOURCE,
		f.config.deployment.as_str(),
		&format!("oidc:{identity}"),
		&continued_body,
		Some(proof.clone()),
		now,
		false,
	)
	.await;
	let (status, continued) = post(&app, &raw, &continued_body).await;
	assert_eq!(
		status, 200,
		"crypto-only update preserves cursor: {continued}"
	);
	assert_eq!(
		f.viewer_grant(
			&TenantName::new("acme").unwrap(),
			&DeploymentId::parse(SOURCE).unwrap(),
			identity
		)
		.await
		.unwrap()
		.unwrap()
		.revision,
		1
	);

	let mut restarted = f.clone();
	restarted.store = aidash_server::store::Store::from_pool(
		sqlx::PgPool::connect(&url).await.unwrap(),
		f.config.deployment.clone(),
	)
	.await
	.unwrap();
	common::peering::carry_over(&mut restarted, &f);
	let after_overlap = now + 604_800;
	let clock = Clock(&restarted, after_overlap);
	let raw = token(
		&next,
		&sender.to_string(),
		&receiver.to_string(),
		"source-subject",
		&body,
		None,
		after_overlap,
		false,
	)
	.await;
	let body_bytes = body.to_string();
	let digest = RequestDigest {
		method: "POST",
		path: "/scoped/graph",
		body: body_bytes.as_bytes(),
	};
	let admitted = inbound::accept(
		inbound::Inbound {
			receiver: receiver.clone(),
			sender: sender.clone(),
			subject: "source-subject".into(),
			proof: inbound::Proof::Token(raw),
		},
		&digest,
		&clock,
	)
	.await
	.unwrap();
	assert_eq!(admitted.sender, sender);
	let old_raw = token(
		&Signing(&predecessor),
		&sender.to_string(),
		&receiver.to_string(),
		"source-subject",
		&body,
		None,
		after_overlap,
		false,
	)
	.await;
	assert!(matches!(
		inbound::accept(
			inbound::Inbound {
				receiver: receiver.clone(),
				sender: sender.clone(),
				subject: "source-subject".into(),
				proof: inbound::Proof::Token(old_raw)
			},
			&digest,
			&clock
		)
		.await,
		Err(aidash_application::Error::Unauthorized)
	));
	let grant_body = graph(
		json!({"kind":"deployment_administrator","id":identity}),
		2,
		Value::Null,
	);
	let grant_raw = token(
		&next,
		SOURCE,
		f.config.deployment.as_str(),
		&format!("oidc:{identity}"),
		&grant_body,
		None,
		after_overlap,
		false,
	)
	.await;
	let grant_bytes = grant_body.to_string();
	let grant_digest = RequestDigest {
		method: "POST",
		path: "/scoped/graph",
		body: grant_bytes.as_bytes(),
	};
	let grant_admitted = inbound::accept_grant_viewer(
		inbound::ViewerInbound {
			receiver: f.config.deployment.clone(),
			viewer: DeploymentId::parse(SOURCE).unwrap(),
			tenant: TenantName::new("acme").unwrap(),
			identity,
			token: grant_raw,
		},
		&grant_digest,
		&clock,
	)
	.await
	.unwrap();
	assert_eq!(grant_admitted.grant_revision, 1);
	let grant_keys = restarted
		.receiving_keys(&grant_owner)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(grant_keys.keys.current.key_id.as_str(), next.kid());
	assert_eq!(grant_keys.keys_revision, 2);

	// An independent unlearned grant missed the overlap, so the old proof is
	// refused even though its outer token is fresh and correctly request-bound.
	let missed = Uuid::new_v4();
	let initial = serde_json::to_string(&before.keys).unwrap();
	sqlx::query("INSERT INTO authorization_peer_graph_grants(tenant,source_deployment,source_identity,source_key_id,source_public_key_pem,source_keys,enabled,revision) VALUES('acme',$1,$2,$3,$4,$5,true,1)")
        .bind(SOURCE).bind(missed).bind(predecessor.key_id().to_string()).bind(predecessor.public_key_pem()).bind(initial).execute(f.store.pool.fixture_driver()).await.unwrap();
	let body = graph(
		json!({"kind":"deployment_administrator","id":missed}),
		2,
		Value::Null,
	);
	let raw = token(
		&next,
		SOURCE,
		f.config.deployment.as_str(),
		&format!("oidc:{missed}"),
		&body,
		Some(proof),
		after_overlap,
		false,
	)
	.await;
	let body_bytes = body.to_string();
	let digest = RequestDigest {
		method: "POST",
		path: "/scoped/graph",
		body: body_bytes.as_bytes(),
	};
	assert!(matches!(
		inbound::accept_grant_viewer(
			inbound::ViewerInbound {
				receiver: f.config.deployment.clone(),
				viewer: DeploymentId::parse(SOURCE).unwrap(),
				tenant: TenantName::new("acme").unwrap(),
				identity: missed,
				token: raw
			},
			&digest,
			&clock
		)
		.await,
		Err(aidash_application::Error::Unauthorized)
	));
	restarted.store.pool.close().await;
	restarted.store.control_pool.close().await;
	drop(app);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn receiving_cas_preserves_concurrent_disable_quarantine_and_newer_keys(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	bootstrap(&f, &app, "http://localhost:1").await;
	let source = common::register_source(&f, &app, SOURCE, "acme").await;
	let owner = ReceivingKeyOwner::Registration {
		tenant: TenantName::new("acme").unwrap(),
		node: source.receiver.node.clone(),
		source_node: NodeAddress::parse(&source.address()).unwrap(),
	};
	let initial = f.receiving_keys(&owner).await.unwrap().unwrap();
	let next = successor();
	let mut candidate = initial.keys.clone();
	candidate
		.accept_successor(
			&initial.keys.current.key_id,
			PublicDeploymentKey::new(
				DeploymentKeyId::parse(next.kid()).unwrap(),
				next.public_key().to_bytes(),
			)
			.unwrap(),
			RolloverSchedule::new(chrono::Utc::now() - chrono::Duration::hours(25)).unwrap(),
			chrono::Utc::now(),
		)
		.unwrap();
	let (one, two) = tokio::join!(
		f.advance_keys(&owner, initial.keys_revision, 1, &candidate),
		f.advance_keys(&owner, initial.keys_revision, 1, &candidate)
	);
	assert_ne!(one.unwrap(), two.unwrap(), "exactly one CAS wins");
	let current = f.receiving_keys(&owner).await.unwrap().unwrap();
	assert_eq!(current.keys_revision, initial.keys_revision + 1);
	assert!(
		!f.advance_keys(&owner, initial.keys_revision, 1, &candidate)
			.await
			.unwrap()
	);

	let name = NodeName::derived("permission-edit").to_string();
	let source_node = source.remote.node(&name);
	let path = common::peering::registration_path("acme", source.receiver.node.as_str());
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		&path,
		source.remote.registration(&name),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let edit_owner = ReceivingKeyOwner::Registration {
		tenant: TenantName::new("acme").unwrap(),
		node: source.receiver.node.clone(),
		source_node: NodeAddress::parse(&source_node).unwrap(),
	};
	let mut blocker = f.store.pool.fixture_driver().begin().await.unwrap();
	let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
		.fetch_one(&mut *blocker)
		.await
		.unwrap();
	sqlx::query(
		"SELECT 1 FROM node_registrations WHERE tenant='acme' AND source_node=$1 FOR UPDATE",
	)
	.bind(&source_node)
	.fetch_one(&mut *blocker)
	.await
	.unwrap();
	let owned_f = f.clone();
	let owned_owner = edit_owner.clone();
	let owned_candidate = candidate.clone();
	let advancement = tokio::spawn(async move {
		owned_f
			.advance_keys(&owned_owner, 1, 1, &owned_candidate)
			.await
	});
	common::wait_for_blocked(&f.store.pool, pid, 1).await;
	let mut input = source.remote.registration(&name);
	input["expected_revision"] = json!(1);
	input["atomic_transactions"] = json!(true);
	let owned_app = app.clone();
	let bearer = f.config.api_token.clone();
	let edit =
		tokio::spawn(async move { request(&owned_app, &bearer, "POST", &path, input).await });
	common::wait_for_blocked(&f.store.pool, pid, 2).await;
	blocker.commit().await.unwrap();
	assert!(advancement.await.unwrap().unwrap());
	let (status, edited) = edit.await.unwrap();
	assert_eq!(status, 200, "{edited}");
	assert_eq!(edited["key_id"], next.kid());
	assert_eq!(edited["revision"], 2);
	assert_eq!(edited["atomic_transactions"], true);
	let retained = f.receiving_keys(&edit_owner).await.unwrap().unwrap();
	assert_eq!(retained.keys.current.key_id.as_str(), next.kid());
	assert_eq!(retained.keys_revision, 2);

	// Each negative control uses fresh, valid predecessor trust and candidate,
	// so refusing cannot be explained by a duplicate or retired predecessor.

	for failure in ["auth_revision", "keys_revision", "disable", "quarantine"] {
		let name = NodeName::derived(failure).to_string();
		let source_node = source.remote.node(&name);
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			&common::peering::registration_path("acme", source.receiver.node.as_str()),
			source.remote.registration(&name),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let scoped_owner = ReceivingKeyOwner::Registration {
			tenant: TenantName::new("acme").unwrap(),
			node: source.receiver.node.clone(),
			source_node: NodeAddress::parse(&source_node).unwrap(),
		};
		match failure {
			"auth_revision" => {
				sqlx::query(
					"UPDATE node_registrations SET revision=2 WHERE tenant='acme' AND source_node=$1",
				)
				.bind(&source_node)
				.execute(f.store.pool.fixture_driver())
				.await
				.unwrap();
				assert!(
					!f.advance_keys(&scoped_owner, 1, 1, &candidate)
						.await
						.unwrap()
				);
				assert!(
					f.advance_keys(&scoped_owner, 1, 2, &candidate)
						.await
						.unwrap()
				);
			}
			"keys_revision" => {
				sqlx::query(
					"UPDATE node_registrations SET keys_revision=2 WHERE tenant='acme' AND source_node=$1",
				)
				.bind(&source_node)
				.execute(f.store.pool.fixture_driver())
				.await
				.unwrap();
				assert!(
					!f.advance_keys(&scoped_owner, 1, 1, &candidate)
						.await
						.unwrap()
				);
				assert!(
					f.advance_keys(&scoped_owner, 2, 1, &candidate)
						.await
						.unwrap()
				);
			}
			_ => {
				let mut writer = f.store.pool.fixture_driver().begin().await.unwrap();
				let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
					.fetch_one(&mut *writer)
					.await
					.unwrap();
				let mut quarantined = initial.keys.clone();
				quarantined.quarantine();
				if failure == "disable" {
					sqlx::query("UPDATE node_registrations SET enabled=false,revision=2 WHERE tenant='acme' AND source_node=$1").bind(&source_node).execute(&mut *writer).await.unwrap();
				} else {
					sqlx::query("UPDATE node_registrations SET source_keys=$1,keys_revision=2 WHERE tenant='acme' AND source_node=$2").bind(serde_json::to_string(&quarantined).unwrap()).bind(&source_node).execute(&mut *writer).await.unwrap();
				}
				let owned_f = f.clone();
				let owned_owner = scoped_owner.clone();
				let owned_candidate = candidate.clone();
				let stale = tokio::spawn(async move {
					owned_f
						.advance_keys(&owned_owner, 1, 1, &owned_candidate)
						.await
				});
				common::wait_for_blocked(&f.store.pool, pid, 1).await;
				writer.commit().await.unwrap();
				assert!(!stale.await.unwrap().unwrap(), "{failure}");
				if failure == "disable" {
					assert!(
						!f.advance_keys(&scoped_owner, 1, 2, &candidate)
							.await
							.unwrap()
					);
					assert_eq!(
						f.receiving_keys(&scoped_owner).await.unwrap().unwrap().keys,
						initial.keys
					);
				} else {
					assert!(
						!f.advance_keys(&scoped_owner, 2, 1, &candidate)
							.await
							.unwrap()
					);
					assert_eq!(
						f.receiving_keys(&scoped_owner).await.unwrap().unwrap().keys,
						quarantined
					);
				}
			}
		}
	}

	drop(source);
	drop(app);
	cleanup(f, &url, &schema).await;
}

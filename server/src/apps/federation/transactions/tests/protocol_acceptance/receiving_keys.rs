//! Real durable recovery owns receiving-key persistence, not a synthetic journal.
use super::*;
use aidash_application::{
	federation::{
		inbound::{self, Inbound, Proof, RequestDigest},
		key::Signing,
	},
	ports::federation::inbound::{InboundRepository, ReceivingKeyOwner},
};
use aidash_capability::{
	InMemorySigner, TokenSigner,
	federation::{
		FederationClaims, body_digest,
		rollover::{RolloverClaims, mint_transition},
	},
};
use aidash_domain::{nodes::DeploymentKeyId, tenancy::TenantName};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::ExposeSecret;
#[path = "../../../../execution/tests/support/federation_clock.rs"]
mod federation_clock;
use federation_clock::Clock;

fn request(
	from: &Federation,
	to: &Federation,
	path: &str,
	token: &str,
	body: &[u8],
) -> reqwest::RequestBuilder {
	let mut request = from
		.client
		.post(format!("{}/federation/v0.1{path}", to.config.endpoint))
		.header("content-type", "application/json")
		.body(body.to_vec());
	for (name, value) in common::peering::peer_headers(token) {
		request = request.header(name, value);
	}
	request
}
async fn response(request: reqwest::RequestBuilder) -> (u16, Value) {
	let response = request.send().await.unwrap();
	let status = response.status().as_u16();
	let body = response.json().await.unwrap();
	(status, body)
}

/// Exact terminal recovery can learn while disabled; stale authority and
/// quarantine still fail closed. Both cases use real independently created work.
#[rstest::rstest]
#[case::concurrent_disable(false)]
#[case::concurrent_quarantine(true)]
#[tokio::test]
async fn recovery_successor_persists_for_a_closed_disabled_participant(
	#[future(awt)] pair: Pair,
	#[case] quarantine: bool,
) {
	let (a, mut b, manifest, wa, wb) = pair;
	// A correctly registered, same-Deployment sibling is still not the caller
	// named by this immutable participant journal.
	let (status, sibling) = a
		.request(
			reqwest::Method::POST,
			"/api/tenants/acme/nodes",
			Some(json!({"label":"Unrelated recovery caller"})),
		)
		.await;
	assert_eq!(status, 201, "{sibling}");
	let other =
		a.f.config.deployment.node(
			aidash_domain::nodes::NodeName::parse(sibling["name"].as_str().unwrap()).unwrap(),
		);
	let registration = common::peering::registration(
		&other.to_string(),
		Some((&a.f.config.endpoint, &common::peering::key_id(&a.f))),
	);
	assert_eq!(
		b.request(
			reqwest::Method::POST,
			&common::peering::registration_path("acme", b.node_address().node.as_str()),
			Some(registration)
		)
		.await
		.0,
		200
	);

	coordinator::submit(&a.f, &manifest).await.unwrap();
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	steps(&a, manifest.id, 2).await;
	assert_eq!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("COMMIT")
	);
	b.restart().await;
	for _ in 0..10 {
		if coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.visible
		{
			break;
		}
		coordinator::advance(&a.f, manifest.id).await.unwrap();
	}
	assert!(
		coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.visible
	);
	// A real terminal result whose coordinator has not recorded its final ACK.
	let terminal = participant::finish(&b.f, None, &a.address, &manifest)
		.await
		.unwrap();
	assert_eq!(terminal.phase, "COMMITTED");
	assert!(
		!coordinator::status(&a.f, manifest.id)
			.await
			.unwrap()
			.complete
	);

	let (status, sibling) = b
		.request(
			reqwest::Method::POST,
			"/api/tenants/acme/nodes",
			Some(json!({"label":"Recovery successor sibling"})),
		)
		.await;
	assert_eq!(status, 201, "{sibling}");
	assert_eq!(
		b.request(
			reqwest::Method::POST,
			&format!(
				"/api/tenants/acme/nodes/{}/default",
				sibling["name"].as_str().unwrap()
			),
			None
		)
		.await
		.0,
		200
	);
	let name = b.node_address().node;
	let (status, closed) = b
		.request(
			reqwest::Method::POST,
			&format!("/api/tenants/acme/nodes/{name}/deletion"),
			Some(json!({"confirmation":name})),
		)
		.await;
	assert_eq!(status, 200, "{closed}");

	let raw = InMemorySigner::new("temporary".into(), [21; 32]);
	let next = InMemorySigner::new(
		DeploymentKeyId::of_public_key(&raw.public_key().to_bytes()).to_string(),
		[21; 32],
	);
	let old = a.f.config.deployment_key.as_deref().unwrap();
	let now = Utc::now().timestamp() as u64;
	let transition = mint_transition(
		&RolloverClaims {
			deployment: a.f.config.deployment.to_string(),
			predecessor_kid: old.key_id().to_string(),
			successor_kid: next.kid().into(),
			successor_public_key: URL_SAFE_NO_PAD.encode(next.public_key().to_bytes()),
			published_at: now - 90_000,
			activates_at: now - 3_600,
			retires_at: now - 3_600 + 604_800,
		},
		&Signing(old),
	)
	.await
	.unwrap();
	let body = serde_json::to_vec(&manifest).unwrap();
	let claims = FederationClaims {
		iss: a.f.config.deployment.to_string(),
		sub: a.address.clone(),
		aud: b.address.clone(),
		subject: String::new(),
		iat: now,
		exp: now + 60,
		jti: Uuid::new_v4(),
		kid: next.kid().into(),
		protocol: aidash_domain::federation::protocol::CURRENT.into(),
		method: "POST".into(),
		path: "/transactions/finish".into(),
		body: body_digest(&body),
		transition: Some(transition),
	};
	let tenant = TenantName::new("acme").unwrap();
	let owner = ReceivingKeyOwner::Registration {
		tenant: tenant.clone(),
		node: b.node_address().node,
		source_node: a.node_address(),
	};
	let initial = b.f.receiving_keys(&owner).await.unwrap().unwrap();
	assert_eq!(initial.keys_revision, 1);
	let other_owner = ReceivingKeyOwner::Registration {
		tenant: tenant.clone(),
		node: name.clone(),
		source_node: other.clone(),
	};
	let other_initial = b.f.receiving_keys(&other_owner).await.unwrap().unwrap();
	// Every negative carries a valid successor proof and a correctly signed
	// outer request; the durable owner must refuse BEFORE any key write.
	let mut changed = manifest.clone();
	changed.deadline += Duration::seconds(1);
	let mut fresh_json = json!(manifest);
	fresh_json["id"] = json!(Uuid::new_v4());
	for participant in fresh_json["participants"].as_array_mut().unwrap() {
		participant["mutations"][0]["expected_revision"] = json!(1);
	}
	let fresh: Manifest = serde_json::from_value(fresh_json).unwrap();
	let other_address = other.to_string();
	for (path, manifest, sender) in [
		("/transactions/finish", &changed, a.address.as_str()),
		("/transactions/finish", &manifest, other_address.as_str()),
		("/transactions/reserve", &fresh, a.address.as_str()),
		("/transactions/prepare", &fresh, a.address.as_str()),
	] {
		let bytes = serde_json::to_vec(manifest).unwrap();
		let denied = FederationClaims {
			sub: sender.to_owned(),
			path: path.into(),
			body: body_digest(&bytes),
			jti: Uuid::new_v4(),
			..claims.clone()
		}
		.mint(&next)
		.await
		.unwrap();
		assert_eq!(
			response(request(&a.f, &b.f, path, denied.expose_secret(), &bytes))
				.await
				.0,
			401
		);
		assert_eq!(b.f.receiving_keys(&owner).await.unwrap().unwrap(), initial);
	}
	assert_eq!(
		b.f.receiving_keys(&other_owner).await.unwrap().unwrap(),
		other_initial
	);
	let wrong_binding = FederationClaims {
		method: "DELETE".into(),
		jti: Uuid::new_v4(),
		..claims.clone()
	}
	.mint(&next)
	.await
	.unwrap();
	assert_eq!(
		response(request(
			&a.f,
			&b.f,
			&claims.path,
			wrong_binding.expose_secret(),
			&body
		))
		.await
		.0,
		401
	);
	assert_eq!(b.f.receiving_keys(&owner).await.unwrap().unwrap(), initial);

	let mut disable = common::peering::registration(&a.address, None);
	disable["enabled"] = json!(false);
	disable["atomic_transactions"] = json!(false);
	disable["expected_revision"] = json!(1);
	let registration_path = common::peering::registration_path("acme", name.as_str());
	if quarantine {
		let (status, value) = b
			.request(
				reqwest::Method::POST,
				&registration_path,
				Some(disable.clone()),
			)
			.await;
		assert_eq!(status, 200, "{value}");
	}
	// The fully verified candidate waits at its real CAS capacity lock.
	// Ordinary registration updates do not take this lock, so a real disable
	// can commit before the candidate compares its captured revisions.
	let pool = b.f.store.pool.fixture_driver();
	let mut barrier = pool.begin().await.unwrap();
	let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
		.fetch_one(&mut *barrier)
		.await
		.unwrap();
	sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('nodes:acme',24100241))")
		.execute(&mut *barrier)
		.await
		.unwrap();
	let token = claims.mint(&next).await.unwrap();
	let queued = request(&a.f, &b.f, &claims.path, token.expose_secret(), &body);
	let pending = tokio::spawn(response(queued));
	common::wait_for_blocked(&b.f.store.pool, blocker, 1).await;
	if !quarantine {
		let (status, value) = b
			.request(reqwest::Method::POST, &registration_path, Some(disable))
			.await;
		assert_eq!(status, 200, "{value}");
	}
	if quarantine {
		// Manual controls are separately owned8b. This isolated row mutation
		// models a winning quarantine after full HTTP verification, not a fake
		// participant or a production proof that clears quarantine.
		let mut held = initial.keys.clone();
		held.quarantine();
		sqlx::query("UPDATE node_registrations SET source_keys=$1,keys_revision=keys_revision+1 WHERE tenant='acme' AND node=$2 AND source_node=$3")
            .bind(serde_json::to_string(&held).unwrap()).bind(name.as_str()).bind(&a.address).execute(pool).await.unwrap();
	}
	barrier.commit().await.unwrap();
	assert_eq!(
		pending.await.unwrap().0,
		401,
		"stale authority/quarantine cannot promote captured keys"
	);
	let before =
		b.f.registration(&tenant, &name, &a.node_address())
			.await
			.unwrap()
			.unwrap();
	assert!(!before.enabled);
	assert!(!before.atomic_transactions);
	assert_eq!(before.revision, 2);
	let retained = b.f.receiving_keys(&owner).await.unwrap().unwrap();
	assert_eq!(retained.keys.current.key_id, initial.keys.current.key_id);
	assert_eq!(retained.keys_revision, if quarantine { 2 } else { 1 });
	let token = FederationClaims {
		jti: Uuid::new_v4(),
		..claims.clone()
	}
	.mint(&next)
	.await
	.unwrap();
	let (status, value) = response(request(
		&a.f,
		&b.f,
		&claims.path,
		token.expose_secret(),
		&body,
	))
	.await;
	if quarantine {
		assert_eq!(status, 401, "{value}");
		assert!(
			b.f.receiving_keys(&owner)
				.await
				.unwrap()
				.unwrap()
				.keys
				.quarantined
		);
	} else {
		assert_eq!(
			status, 200,
			"fresh exact recovery must learn while still disabled: {value}"
		);
		let learned = b.f.receiving_keys(&owner).await.unwrap().unwrap();
		assert_eq!(learned.keys.current.key_id.as_str(), next.kid());
		assert_eq!(learned.keys_revision, 2);
		b.restart().await;
		assert_eq!(b.f.receiving_keys(&owner).await.unwrap().unwrap(), learned);
		// Only time is virtual. All authority/key reads use the restarted real
		// adapter, then the actual participant owner retrieves the coordinator
		// decision over HTTP and returns its terminal outcome again.
		let after = now - 3_600 + 604_800 + 1;
		let future = FederationClaims {
			iat: after,
			exp: after + 60,
			jti: Uuid::new_v4(),
			transition: None,
			..claims.clone()
		};
		let token = future.mint(&next).await.unwrap();
		let digest = RequestDigest {
			method: "POST",
			path: "/transactions/finish",
			body: &body,
		};
		let admitted = inbound::accept(
			Inbound {
				receiver: b.node_address(),
				sender: a.node_address(),
				subject: String::new(),
				proof: Proof::Token(token.expose_secret().to_owned()),
			},
			&digest,
			&Clock(&b.f, after),
		)
		.await
		.unwrap();
		let terminal = participant::finish(&b.f, Some(&admitted), &a.address, &manifest)
			.await
			.unwrap();
		assert_eq!(terminal.phase, "COMMITTED");
		let old_token = FederationClaims {
			kid: old.key_id().to_string(),
			jti: Uuid::new_v4(),
			..future
		}
		.mint(&Signing(old))
		.await
		.unwrap();
		assert!(matches!(
			inbound::accept(
				Inbound {
					receiver: b.node_address(),
					sender: a.node_address(),
					subject: String::new(),
					proof: Proof::Token(old_token.expose_secret().to_owned())
				},
				&digest,
				&Clock(&b.f, after)
			)
			.await,
			Err(aidash_application::Error::Unauthorized)
		));
		assert!(complete(&a, manifest.id).await.complete);
		for (node, id) in [(&a, wa), (&b, wb)] {
			assert_eq!(
				node.f
					.store
					.workspace(&TEST_FIXTURE, id)
					.await
					.unwrap()
					.revision,
				1
			);
		}
		assert_eq!(b.f.receiving_keys(&owner).await.unwrap().unwrap(), learned);
	}
	let after =
		b.f.registration(&tenant, &name, &a.node_address())
			.await
			.unwrap()
			.unwrap();
	assert_eq!(
		(
			after.enabled,
			after.atomic_transactions,
			after.revision,
			after.actor,
			after.created_at,
			after.updated_at
		),
		(
			before.enabled,
			before.atomic_transactions,
			before.revision,
			before.actor,
			before.created_at,
			before.updated_at
		)
	);
	assert_eq!(
		b.f.store
			.workspace(&TEST_FIXTURE, wb)
			.await
			.unwrap()
			.revision,
		1,
		"terminal retry has exactly one effect"
	);
	a.cleanup().await;
	b.cleanup().await;
}

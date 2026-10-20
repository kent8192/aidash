use super::*;
use crate::ports::federation::inbound::ViewerGrant;
use aidash_capability::{FederationClaims, InMemorySigner, body_digest};
use aidash_domain::{
	identity::peer_mapping::Mapping,
	nodes::{
		DeploymentKeyId, NodeName,
		registration::{NodeRegistration, PinnedKey},
	},
};
use async_trait::async_trait;
use chrono::Utc;
use secrecy::ExposeSecret;
use std::sync::Mutex;

const OWN: &str = "aidash://receiver";
const OTHER: &str = "aidash://sender";
const NOW: u64 = 1_000;

fn node(name: &str) -> NodeName {
	NodeName::parse(name).unwrap()
}

fn address(deployment: &str, name: &str) -> NodeAddress {
	NodeAddress::parse(&format!("{deployment}/{name}")).unwrap()
}

fn tenant(name: &str) -> TenantName {
	TenantName::new(name).unwrap()
}

// Node names are `<adjective>-<noun>-<8 characters>`.
const RECEIVING: &str = "calm-heron-aaaaaaaa";
const SIBLING: &str = "calm-heron-bbbbbbbb";
const NEIGHBOUR: &str = "brave-otter-cccccccc";
const SENDING: &str = "brave-otter-dddddddd";
const TENANT: &str = "calm-heron-t1t1t1t1";
const NEIGHBOURS: &str = "brave-otter-t2t2t2t2";

fn signer(seed: u8) -> InMemorySigner {
	InMemorySigner::new("kid".into(), [seed; 32])
}

fn key_id(signer: &InMemorySigner) -> DeploymentKeyId {
	DeploymentKeyId::of_public_key(&signer.public_key().to_bytes())
}

struct World {
	own: DeploymentId,
	nodes: Vec<(NodeName, TenantName)>,
	deleted: Vec<TenantName>,
	registrations: Vec<NodeRegistration>,
	mappings: Vec<Mapping>,
	may_use: bool,
	enabled: bool,
	key: bool,
	grant: Option<ViewerGrant>,
	asked: Mutex<Vec<&'static str>>,
}

impl World {
	fn new() -> Self {
		Self {
			own: DeploymentId::parse(OWN).unwrap(),
			nodes: vec![
				(node(RECEIVING), tenant(TENANT)),
				(node(SIBLING), tenant(TENANT)),
				(node(NEIGHBOUR), tenant(NEIGHBOURS)),
			],
			deleted: vec![],
			registrations: vec![],
			mappings: vec![],
			may_use: true,
			enabled: true,
			key: true,
			grant: None,
			asked: Mutex::new(vec![]),
		}
	}

	fn register(&mut self, sender: NodeAddress, counterpart: Counterpart, enabled: bool) {
		self.registrations.push(NodeRegistration {
			tenant: tenant(TENANT),
			node: node(RECEIVING),
			source_node: sender,
			counterpart,
			atomic_transactions: false,
			enabled,
			revision: 1,
			actor: "test".into(),
			created_at: Utc::now(),
			updated_at: Utc::now(),
		});
	}

	fn map(&mut self, sender: NodeAddress, subject: &str, enabled: bool) {
		self.mappings.push(Mapping {
			node: node(RECEIVING),
			source_node: sender,
			source_subject: subject.into(),
			tenant: TENANT.into(),
			credential_id: Uuid::from_u128(7),
			enabled,
			revision: 1,
			actor: "test".into(),
			updated_at: Utc::now(),
		});
	}
}

#[async_trait]
impl InboundRepository for World {
	fn deployment(&self) -> &DeploymentId {
		&self.own
	}
	fn federation_enabled(&self) -> bool {
		self.enabled
	}
	fn holds_key(&self) -> bool {
		self.key
	}
	fn now(&self) -> u64 {
		NOW
	}
	async fn node_tenant(&self, name: &NodeName) -> Result<Option<TenantName>> {
		self.asked.lock().unwrap().push("node_tenant");
		Ok(self
			.nodes
			.iter()
			.find(|(held, _)| held == name)
			.map(|(_, tenant)| tenant.clone()))
	}
	async fn tenant_live(&self, tenant: &TenantName) -> Result<bool> {
		Ok(!self.deleted.contains(tenant))
	}
	async fn registration(
		&self,
		tenant: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
	) -> Result<Option<NodeRegistration>> {
		self.asked.lock().unwrap().push("registration");
		Ok(self
			.registrations
			.iter()
			.find(|row| {
				row.tenant == *tenant && row.node == *receiver && row.source_node == *sender
			})
			.cloned())
	}
	async fn mapping(
		&self,
		_: &TenantName,
		receiver: &NodeName,
		sender: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>> {
		self.asked.lock().unwrap().push("mapping");
		Ok(self
			.mappings
			.iter()
			.find(|row| {
				row.node == *receiver && row.source_node == *sender && row.source_subject == subject
			})
			.cloned())
	}
	async fn subject_may_use(&self, _: &TenantName, _: &NodeName, _: &str) -> Result<bool> {
		self.asked.lock().unwrap().push("policy");
		Ok(self.may_use)
	}
	async fn viewer_grant(
		&self,
		_: &TenantName,
		_: &DeploymentId,
		_: Uuid,
	) -> Result<Option<ViewerGrant>> {
		Ok(self.grant.clone())
	}
}

fn digest() -> RequestDigest<'static> {
	RequestDigest {
		method: "POST",
		path: "/scoped/discover",
		body: b"{}",
	}
}

fn local(sender: NodeAddress, subject: &str) -> Inbound {
	Inbound {
		receiver: address(OWN, RECEIVING),
		sender,
		subject: subject.into(),
		proof: Proof::InProcess,
	}
}

async fn token(
	signer: &InMemorySigner,
	sender: &NodeAddress,
	subject: &str,
	change: impl FnOnce(&mut FederationClaims),
) -> Inbound {
	let mut claims = FederationClaims {
		iss: OTHER.into(),
		sub: sender.to_string(),
		aud: address(OWN, RECEIVING).to_string(),
		subject: subject.into(),
		iat: NOW,
		exp: NOW + 60,
		jti: Uuid::new_v4(),
		kid: key_id(signer).as_str().into(),
		protocol: CURRENT.into(),
		method: "POST".into(),
		path: "/scoped/discover".into(),
		body: body_digest(b"{}"),
	};
	change(&mut claims);
	let signing = InMemorySigner::new(key_id(signer).as_str().into(), seed_of(signer));
	let raw = claims
		.mint(&signing)
		.await
		.unwrap()
		.expose_secret()
		.to_owned();
	Inbound {
		receiver: address(OWN, RECEIVING),
		sender: sender.clone(),
		subject: subject.into(),
		proof: Proof::Token(raw),
	}
}

fn seed_of(signer: &InMemorySigner) -> [u8; 32] {
	// The test signers are built from one repeated byte.
	let public = signer.public_key().to_bytes();
	[1, 2, 3, 4, 5, 6, 7, 8]
		.into_iter()
		.map(|seed| (seed, InMemorySigner::new("k".into(), [seed; 32])))
		.find(|(_, candidate)| candidate.public_key().to_bytes() == public)
		.map(|(seed, _)| [seed; 32])
		.expect("a test signer")
}

fn remote(world: &mut World, signer: &InMemorySigner) -> NodeAddress {
	let sender = address(OTHER, SENDING);
	world.register(
		sender.clone(),
		Counterpart::OtherDeployment {
			endpoint: "https://sender.example.com".into(),
			key: PinnedKey {
				key_id: key_id(signer),
				public_key_pem: signer.public_key_pem(),
			},
		},
		true,
	);
	sender
}

#[tokio::test]
async fn a_request_for_a_node_of_another_deployment_is_refused() {
	let world = World::new();
	let mut inbound = local(address(OWN, NEIGHBOUR), "alice");
	inbound.receiver = address("aidash://elsewhere", RECEIVING);
	assert!(matches!(
		accept(inbound, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

#[tokio::test]
async fn an_unknown_receiving_node_and_a_deleted_tenant_look_alike() {
	let mut world = World::new();
	let mut unknown = local(address(OWN, NEIGHBOUR), "alice");
	unknown.receiver = address(OWN, "calm-heron-zzzzzzzz");
	assert!(matches!(
		accept(unknown, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	world.deleted.push(tenant(TENANT));
	assert!(matches!(
		accept(local(address(OWN, NEIGHBOUR), "alice"), &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

#[tokio::test]
async fn a_deleted_sending_tenant_is_refused_inside_the_deployment() {
	let mut world = World::new();
	let sender = address(OWN, NEIGHBOUR);
	world.register(sender.clone(), Counterpart::SameDeployment, true);
	world.deleted.push(tenant(NEIGHBOURS));
	assert!(matches!(
		accept(local(sender, ""), &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

#[tokio::test]
async fn a_token_never_comes_from_this_deployment_and_a_process_never_speaks_for_another() {
	let mut world = World::new();
	let signer = signer(1);
	let sender = remote(&mut world, &signer);
	let from_here = token(&signer, &address(OWN, NEIGHBOUR), "alice", |_| {}).await;
	assert!(matches!(
		accept(from_here, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	let mut other = local(sender, "alice");
	other.proof = Proof::InProcess;
	assert!(matches!(
		accept(other, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

#[tokio::test]
async fn another_tenant_inside_the_deployment_needs_a_registration_and_a_mapping() {
	let mut world = World::new();
	let sender = address(OWN, NEIGHBOUR);
	let unregistered = accept(local(sender.clone(), "alice"), &digest(), &world).await;
	assert!(
		matches!(unregistered, Err(Error::Unauthorized)),
		"{unregistered:?}"
	);

	world.register(sender.clone(), Counterpart::SameDeployment, true);
	let unmapped = accept(local(sender.clone(), "alice"), &digest(), &world).await;
	assert!(matches!(unmapped, Err(Error::Forbidden)), "{unmapped:?}");

	world.map(sender.clone(), "alice", false);
	let disabled = accept(local(sender.clone(), "alice"), &digest(), &world).await;
	assert!(matches!(disabled, Err(Error::Forbidden)), "{disabled:?}");

	world.mappings.clear();
	world.map(sender.clone(), "alice", true);
	let admitted = accept(local(sender.clone(), "alice"), &digest(), &world)
		.await
		.unwrap();
	assert_eq!(
		admitted,
		Admitted {
			tenant: tenant(TENANT),
			receiver: address(OWN, RECEIVING),
			sender: sender.clone(),
			acting: Acting::Mapped {
				credential_id: Uuid::from_u128(7),
				subject: "alice".into()
			},
		}
	);
	assert_eq!(admitted.subject(), Some("alice"));
	let node = accept(local(sender.clone(), ""), &digest(), &world)
		.await
		.unwrap();
	assert_eq!(node.acting, Acting::Node);
	assert_eq!(node.subject(), None);
	world.registrations[0].enabled = false;
	assert!(matches!(
		accept(local(sender, "alice"), &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

#[tokio::test]
async fn within_a_tenant_the_policy_decides_and_nothing_is_registered() {
	let mut world = World::new();
	let sender = address(OWN, SIBLING);
	let admitted = accept(local(sender.clone(), "alice"), &digest(), &world)
		.await
		.unwrap();
	assert_eq!(
		admitted.acting,
		Acting::SameTenant {
			subject: "alice".into()
		}
	);
	assert!(
		!world.asked.lock().unwrap().contains(&"registration"),
		"no registration is read within a Tenant"
	);
	assert!(
		!world.asked.lock().unwrap().contains(&"mapping"),
		"no Peer Mapping is read within a Tenant"
	);
	assert_eq!(
		accept(local(sender.clone(), ""), &digest(), &world)
			.await
			.unwrap()
			.acting,
		Acting::Node
	);
	world.may_use = false;
	assert!(matches!(
		accept(local(sender, "alice"), &digest(), &world).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_token_signed_with_the_pinned_key_for_this_request_is_admitted() {
	let mut world = World::new();
	let signer = signer(1);
	let sender = remote(&mut world, &signer);
	world.map(sender.clone(), "alice", true);
	let admitted = accept(
		token(&signer, &sender, "alice", |_| {}).await,
		&digest(),
		&world,
	)
	.await
	.unwrap();
	assert_eq!(admitted.sender, sender);
	assert_eq!(
		admitted.acting,
		Acting::Mapped {
			credential_id: Uuid::from_u128(7),
			subject: "alice".into()
		}
	);
	let node = accept(token(&signer, &sender, "", |_| {}).await, &digest(), &world)
		.await
		.unwrap();
	assert_eq!(node.acting, Acting::Node);
}

#[tokio::test]
async fn every_way_a_token_can_be_wrong_answers_unauthorized() {
	let mut world = World::new();
	let signer = signer(1);
	let sender = remote(&mut world, &signer);
	world.map(sender.clone(), "alice", true);
	let other = self::signer(2);
	let wrong_body = RequestDigest {
		body: b"{\"search\":{}}",
		..digest()
	};
	let wrong_path = RequestDigest {
		path: "/scoped/execution/inspect",
		..digest()
	};
	let cases: Vec<(&str, Inbound, RequestDigest<'_>)> = vec![
		(
			"another key",
			token(&other, &sender, "alice", |_| {}).await,
			digest(),
		),
		(
			"another body",
			token(&signer, &sender, "alice", |_| {}).await,
			wrong_body,
		),
		(
			"another path",
			token(&signer, &sender, "alice", |_| {}).await,
			wrong_path,
		),
		(
			"expired",
			token(&signer, &sender, "alice", |claims| {
				claims.iat = NOW - 120;
				claims.exp = NOW - 60;
			})
			.await,
			digest(),
		),
		(
			"issued by another Deployment",
			token(&signer, &sender, "alice", |claims| {
				claims.iss = "aidash://stranger".into()
			})
			.await,
			digest(),
		),
		(
			"another receiver",
			token(&signer, &sender, "alice", |claims| {
				claims.aud = address(OWN, SIBLING).to_string()
			})
			.await,
			digest(),
		),
		(
			"another subject",
			token(&signer, &sender, "alice", |claims| {
				claims.subject = "mallory".into()
			})
			.await,
			digest(),
		),
	];
	for (name, inbound, request) in cases {
		let result = accept(inbound, &request, &world).await;
		assert!(
			matches!(result, Err(Error::Unauthorized)),
			"{name}: {result:?}"
		);
	}
}

#[tokio::test]
async fn the_switch_and_the_key_gate_every_token() {
	let signer = signer(1);
	for (enabled, key) in [(false, true), (true, false)] {
		let mut world = World::new();
		let sender = remote(&mut world, &signer);
		world.map(sender.clone(), "alice", true);
		world.enabled = enabled;
		world.key = key;
		let result = accept(
			token(&signer, &sender, "alice", |_| {}).await,
			&digest(),
			&world,
		)
		.await;
		assert!(
			matches!(result, Err(Error::Unauthorized)),
			"{enabled} {key}: {result:?}"
		);
	}
	let mut world = World::new();
	let sender = remote(&mut world, &signer);
	world.enabled = false;
	let local_only = address(OWN, NEIGHBOUR);
	world.register(local_only.clone(), Counterpart::SameDeployment, true);
	world.map(local_only.clone(), "alice", true);
	assert!(
		accept(local(local_only, "alice"), &digest(), &world)
			.await
			.is_ok(),
		"exchange inside the Deployment needs neither the switch nor a key"
	);
	let _ = sender;
}

#[tokio::test]
async fn an_unregistered_sender_and_a_missing_mapping_differ_only_after_the_signature() {
	let mut world = World::new();
	let signer = signer(1);
	let sender = address(OTHER, SENDING);
	let unregistered = accept(
		token(&signer, &sender, "alice", |_| {}).await,
		&digest(),
		&world,
	)
	.await;
	assert!(matches!(unregistered, Err(Error::Unauthorized)));
	let _ = remote(&mut world, &signer);
	let unmapped = accept(
		token(&signer, &sender, "alice", |_| {}).await,
		&digest(),
		&world,
	)
	.await;
	assert!(matches!(unmapped, Err(Error::Forbidden)));
}

fn viewer_request(world: &World) -> ViewerInbound {
	ViewerInbound {
		receiver: world.own.clone(),
		viewer: DeploymentId::parse(OTHER).unwrap(),
		tenant: tenant(TENANT),
		identity: Uuid::from_u128(9),
		token: String::new(),
	}
}

async fn viewer_token(
	signer: &InMemorySigner,
	change: impl FnOnce(&mut FederationClaims),
) -> String {
	let mut claims = FederationClaims {
		iss: OTHER.into(),
		sub: OTHER.into(),
		aud: OWN.into(),
		subject: format!("oidc:{}", Uuid::from_u128(9)),
		iat: NOW,
		exp: NOW + 60,
		jti: Uuid::new_v4(),
		kid: key_id(signer).as_str().into(),
		protocol: CURRENT.into(),
		method: "POST".into(),
		path: "/scoped/discover".into(),
		body: body_digest(b"{}"),
	};
	change(&mut claims);
	let signing = InMemorySigner::new(key_id(signer).as_str().into(), seed_of(signer));
	claims
		.mint(&signing)
		.await
		.unwrap()
		.expose_secret()
		.to_owned()
}

#[tokio::test]
async fn a_named_deployment_administrator_is_admitted_under_its_grant_and_nothing_else() {
	let signer = signer(1);
	let mut world = World::new();
	world.grant = Some(ViewerGrant {
		key: PinnedKey {
			key_id: key_id(&signer),
			public_key_pem: signer.public_key_pem(),
		},
		revision: 4,
	});
	let mut request = viewer_request(&world);
	request.token = viewer_token(&signer, |_| {}).await;
	let admitted = accept_grant_viewer(request.clone(), &digest(), &world)
		.await
		.unwrap();
	assert_eq!(admitted.grant_revision, 4);
	assert_eq!(admitted.identity, Uuid::from_u128(9));

	let mut other_identity = request.clone();
	other_identity.identity = Uuid::from_u128(10);
	assert!(matches!(
		accept_grant_viewer(other_identity, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	let mut as_a_node = request.clone();
	as_a_node.token = viewer_token(&signer, |claims| {
		claims.sub = address(OTHER, SENDING).to_string()
	})
	.await;
	assert!(matches!(
		accept_grant_viewer(as_a_node, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	let mut stranger = request.clone();
	stranger.token = viewer_token(&self::signer(2), |_| {}).await;
	assert!(matches!(
		accept_grant_viewer(stranger, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	world.deleted.push(tenant(TENANT));
	assert!(matches!(
		accept_grant_viewer(request.clone(), &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	world.deleted.clear();
	world.enabled = false;
	assert!(matches!(
		accept_grant_viewer(request.clone(), &digest(), &world).await,
		Err(Error::Unauthorized)
	));
	world.enabled = true;
	world.grant = None;
	assert!(matches!(
		accept_grant_viewer(request, &digest(), &world).await,
		Err(Error::Unauthorized)
	));
}

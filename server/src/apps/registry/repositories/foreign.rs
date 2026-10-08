//! Operator imports use enabled peer records and bounded authenticated replies.
use crate::{Error, Result};
use aidash_application::ports::federation::PeerTransport;
use aidash_domain::{
	federation::Peer,
	registry::bindings::{ForeignAgentSnapshot, QualifiedRef},
};
use reinhardt::db::{backends::TransactionExecutor, orm::OrmExecutor};
use std::sync::Arc;

pub(crate) async fn orm<E: OrmExecutor>(
	db: &mut E,
	node: &str,
	reference: &QualifiedRef,
) -> Result<ForeignAgentSnapshot> {
	let peer =
		crate::apps::federation::peer::models::records::enabled(db, &reference.registry_node)
			.await?;
	fetch(peer, node, reference).await
}

pub(crate) async fn native(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	reference: &QualifiedRef,
) -> Result<ForeignAgentSnapshot> {
	let peer =
		crate::apps::federation::peer::models::Peer::enabled_in(tx, &reference.registry_node)
			.await?
			.ok_or(Error::Forbidden)?;
	fetch(peer, node, reference).await
}

async fn fetch(peer: Peer, node: &str, reference: &QualifiedRef) -> Result<ForeignAgentSnapshot> {
	reference.validate()?;
	if reference.registry_node == node || peer.protocol_version != crate::config::PROTOCOL_VERSION {
		return Err(Error::Forbidden);
	}
	let transport = aidash_integrations::federation::PeerHttp {
		client: reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()?,
		node_id: node.into(),
		protocol_version: crate::config::PROTOCOL_VERSION.into(),
		credentials: Arc::new(crate::bootstrap::peer_credentials()),
	};
	let reply = transport
		.request(
			&peer,
			"GET",
			&format!("/discover/{}/{}/bindings", reference.id, reference.version),
			None,
		)
		.await?;
	if reply.status != 200 {
		return Err(Error::Invalid(
			"foreign Agent Binding closure is unavailable".into(),
		));
	}
	let snapshot: ForeignAgentSnapshot = serde_json::from_value(reply.body)?;
	snapshot.validate()?;
	if snapshot.agent != *reference {
		return Err(Error::Invalid(
			"foreign Agent Binding closure has a different origin".into(),
		));
	}
	Ok(snapshot)
}

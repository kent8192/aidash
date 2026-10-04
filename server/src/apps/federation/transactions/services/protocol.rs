use crate::apps::federation::transactions::models::{
	AtomicParticipant, AtomicPeerTrust, coordinator_records,
};
use crate::apps::federation::transactions::{
	LocalStatus, Manifest, Status, authority, coordinator, participant,
};
use crate::authorization::identity::Actor;
use crate::{Error, Result, federation::Federation};
use futures_util::{StreamExt, stream};
use http::HeaderMap;
use reinhardt::{Depends, injectable};
use std::sync::Arc;
use tokio::sync::Semaphore;

use uuid::Uuid;

use crate::apps::federation::transactions::serializers::protocol::{
	PeerRecovery, TransactionDetails, TransactionTrust, TrustChange,
};

#[derive(Clone)]
pub struct SubmissionCapacity {
	slots: Arc<Semaphore>,
}

#[injectable(scope = "singleton")]
pub async fn provide_submission_capacity() -> SubmissionCapacity {
	SubmissionCapacity {
		slots: Arc::new(Semaphore::new(8)),
	}
}

#[derive(Clone)]
pub struct Transactions {
	runtime: Federation,
	submissions: Arc<Semaphore>,
}

#[injectable(scope = "request")]
pub async fn provide(
	#[inject] runtime: Federation,
	#[inject] submissions: Depends<SubmissionCapacity>,
) -> Transactions {
	Transactions {
		runtime,
		submissions: submissions.slots.clone(),
	}
}

impl Transactions {
	pub async fn submit(&self, actor: Actor, manifest: Manifest) -> Result<Status> {
		// Router clones share eight slots in this application's DI scope, leaving
		// control-pool capacity for inbound preflights from independent Nodes.
		let _permit = self
			.submissions
			.acquire()
			.await
			.map_err(|_| Error::TransactionPending)?;
		match actor {
			Actor::Operator => coordinator::submit(&self.runtime, &manifest).await,
			Actor::Subject(identity) => {
				authority::submit(&self.runtime, &identity, &manifest).await
			}
		}
	}

	pub async fn list(&self, actor: Actor) -> Result<Vec<Status>> {
		let mut visible = Vec::new();
		let mut cursor = None;
		loop {
			let rows = {
				let connection = coordinator::coordinator_connection(&self.runtime)?;
				let identity = if let Actor::Subject(identity) = &actor {
					Some(identity)
				} else {
					None
				};
				coordinator_records::visible_candidates(&mut connection.handle(), identity, cursor)
					.await?
			};
			let exhausted = rows.len() < 200;
			let mut checked = stream::iter(rows.into_iter().map(|row| {
				let actor = &actor;
				async move {
					let result = if let Actor::Subject(identity) = actor {
						authority::manage(&self.runtime, identity, &row, "transaction.read").await
					} else {
						Ok(())
					};
					(row, result)
				}
			}))
			.buffered(8);
			while let Some((row, result)) = checked.next().await {
				cursor = Some((row.created_at, row.id));
				match result {
					Ok(()) => {}
					Err(
						Error::Forbidden
						| Error::Unauthorized
						| Error::NotFound(_)
						| Error::External(_)
						| Error::TransactionPending
						| Error::IdentityStatusUnavailable,
					) => continue,
					Err(error) => return Err(error),
				}
				visible.push(row);
				if visible.len() == 200 {
					break;
				}
			}
			if exhausted || visible.len() == 200 {
				break;
			}
		}
		Ok(visible)
	}
	pub async fn details(&self, actor: Actor, id: Uuid) -> Result<TransactionDetails> {
		if let Actor::Subject(identity) = &actor {
			authority::require_owner(&self.runtime, identity, id).await?;
		}
		let transaction = coordinator::status(&self.runtime, id).await?;
		if let Actor::Subject(identity) = &actor {
			authority::manage(&self.runtime, identity, &transaction, "transaction.read").await?;
		}
		let connection = coordinator::coordinator_connection(&self.runtime)?;
		let mut db = connection.handle();
		Ok(TransactionDetails {
			transaction,
			participants: coordinator_records::votes(&mut db, id).await?,
			history: coordinator_records::audit(&mut db, id).await?,
		})
	}
	pub async fn abort(&self, actor: Actor, id: Uuid) -> Result<Status> {
		if let Actor::Subject(identity) = &actor {
			authority::require_owner(&self.runtime, identity, id).await?;
			let state = coordinator::status(&self.runtime, id).await?;
			authority::manage(&self.runtime, identity, &state, "transaction.abort").await?;
			coordinator::status(&self.runtime, id).await
		} else {
			coordinator::abort(&self.runtime, id).await
		}
	}
	pub async fn participants(&self) -> Result<Vec<LocalStatus>> {
		let connection = coordinator::coordinator_connection(&self.runtime)?;
		AtomicParticipant::page(&mut connection.handle()).await
	}

	pub async fn trust_list(&self) -> Result<Vec<TrustChange>> {
		let rows = {
			let connection = coordinator::coordinator_connection(&self.runtime)?;
			AtomicPeerTrust::page(&mut connection.handle()).await?
		};
		let mut result = Vec::with_capacity(rows.len());
		for trust in rows {
			let pending_transactions = if trust.enabled {
				Vec::new()
			} else {
				authority::pending_peer(&self.runtime, &trust.node_id).await?
			};
			result.push(TrustChange {
				trust,
				pending_transactions,
			});
		}
		Ok(result)
	}
	pub async fn trust(
		&self,
		input: TransactionTrust,
	) -> Result<(reinhardt::StatusCode, TrustChange)> {
		if input.enabled {
			self.runtime.peer(&input.node_id).await?;
		}
		let connection = coordinator::coordinator_connection(&self.runtime)?;
		let trust = AtomicPeerTrust::set(connection.handle(), input).await?;
		let pending_transactions = if trust.enabled {
			Vec::new()
		} else {
			authority::pending_peer(&self.runtime, &trust.node_id).await?
		};
		let status = if pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((
			status,
			TrustChange {
				trust,
				pending_transactions,
			},
		))
	}
	pub(crate) async fn restore_peer(
		&self,
		input: PeerRecovery,
	) -> Result<crate::federation::Peer> {
		let credential = crate::config::peer_secret(&input.credential_env)?;
		let connection = coordinator::coordinator_connection(&self.runtime)?;
		crate::apps::federation::peer::models::records::restore_authentication(
			connection.handle(),
			&input.node_id,
			&input.credential_env,
			&credential,
		)
		.await
	}
	pub(crate) async fn preflight(
		&self,
		headers: HeaderMap,
		input: authority::Preflight,
	) -> Result<serde_json::Value> {
		authority::preflight(
			&self.runtime,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			&input,
		)
		.await?;
		Ok(serde_json::json!({"authorized":true}))
	}
	pub(crate) async fn read_access(
		&self,
		headers: HeaderMap,
		input: authority::Preflight,
	) -> Result<serde_json::Value> {
		authority::read_access(
			&self.runtime,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			&input,
		)
		.await?;
		Ok(serde_json::json!({"authorized":true}))
	}
	pub(crate) async fn authority_ticket(
		&self,
		headers: HeaderMap,
		id: Uuid,
	) -> Result<authority::Preflight> {
		authority::ticket(
			&self.runtime,
			id,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
		)
		.await
	}
	pub async fn reserve(&self, headers: HeaderMap, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		participant::reserve(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			&manifest,
		)
		.await
	}
	pub async fn prepare(&self, headers: HeaderMap, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		participant::prepare(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			&manifest,
		)
		.await
	}
	pub async fn finish(&self, headers: HeaderMap, manifest: Manifest) -> Result<LocalStatus> {
		let f = self.runtime.clone();
		participant::finish(
			&f,
			crate::apps::identity::services::http_auth::peer_node(&headers)?,
			&manifest,
		)
		.await
	}
	pub async fn decision(&self, headers: HeaderMap, id: Uuid) -> Result<Status> {
		let f = self.runtime.clone();
		let state = coordinator::status(&f, id).await?;
		let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
		if !manifest.participants.iter().any(|p| {
			Some(p.node_id.as_str())
				== crate::apps::identity::services::http_auth::peer_node(&headers).ok()
		}) {
			return Err(Error::Forbidden);
		}
		Ok(state)
	}
}

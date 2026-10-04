use crate::apps::federation::transactions::{
	LocalStatus, Manifest, Status, authority, participant,
};
use crate::authorization::identity::Actor;
use crate::{Error, Result, federation::Federation};
use aidash_application::transactions::management as application;
use aidash_domain::identity::execution::ExecutionPrincipal;
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
		// Router clones share eight DI-scoped slots, leaving control capacity
		// for inbound preflights from independent Nodes.
		let _permit = self
			.submissions
			.acquire()
			.await
			.map_err(|_| Error::TransactionPending)?;
		application::submit(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			principal(&actor).as_ref(),
			&manifest,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}

	pub async fn list(&self, actor: Actor) -> Result<Vec<Status>> {
		application::list(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			principal(&actor).as_ref(),
		)
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	pub async fn details(&self, actor: Actor, id: Uuid) -> Result<TransactionDetails> {
		application::details(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			principal(&actor).as_ref(),
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub async fn abort(&self, actor: Actor, id: Uuid) -> Result<Status> {
		application::abort(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			principal(&actor).as_ref(),
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub async fn participants(&self) -> Result<Vec<LocalStatus>> {
		application::participants(&crate::bootstrap::transaction_management_repository(
			&self.runtime,
		))
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	pub async fn trust_list(&self) -> Result<Vec<TrustChange>> {
		application::trust_list(&crate::bootstrap::transaction_management_repository(
			&self.runtime,
		))
		.await
		.map(|rows| rows.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	pub async fn trust(
		&self,
		input: TransactionTrust,
	) -> Result<(reinhardt::StatusCode, TrustChange)> {
		let change = application::trust(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			input.into(),
		)
		.await?;
		let status = if change.pending_transactions.is_empty() {
			reinhardt::StatusCode::OK
		} else {
			reinhardt::StatusCode::ACCEPTED
		};
		Ok((status, change.into()))
	}
	pub(crate) async fn restore_peer(
		&self,
		input: PeerRecovery,
	) -> Result<crate::federation::Peer> {
		application::restore_peer(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			&input.node_id,
			&input.credential_env,
		)
		.await
		.map_err(Into::into)
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
		let caller = crate::apps::identity::services::http_auth::peer_node(&headers).ok();
		application::decision(
			&crate::bootstrap::transaction_management_repository(&self.runtime),
			caller,
			id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
}

fn principal(actor: &Actor) -> Option<ExecutionPrincipal> {
	match actor {
		Actor::Operator => None,
		Actor::Subject(identity) => Some(ExecutionPrincipal {
			credential_id: identity.credential_id,
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		}),
	}
}

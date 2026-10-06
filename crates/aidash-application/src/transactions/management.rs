//! Live disclosure, immutable ownership and trust changes for every management adapter.
use crate::{Error, Result, ports::transactions::management::ManagementRepository};
use aidash_domain::{
	federation::Peer,
	identity::execution::ExecutionPrincipal,
	transactions::{
		Manifest,
		authority::Status,
		coordination::LocalStatus,
		management::{Details, Trust, TrustChange},
	},
};
use futures_util::{StreamExt, stream};
use uuid::Uuid;

/// None denotes operator authority established by a trusted authentication adapter.
pub async fn submit(
	repository: &dyn ManagementRepository,
	identity: Option<&ExecutionPrincipal>,
	manifest: &Manifest,
) -> Result<Status> {
	match identity {
		Some(identity) => repository.submit_subject(identity, manifest).await,
		None => repository.submit_operator(manifest).await,
	}
}

pub async fn list(
	repository: &dyn ManagementRepository,
	identity: Option<&ExecutionPrincipal>,
) -> Result<Vec<Status>> {
	let mut visible = Vec::new();
	let mut cursor = None;
	loop {
		let rows = repository.candidates(identity, cursor).await?;
		let exhausted = rows.len() < 200;
		let mut checked = stream::iter(rows.into_iter().map(|row| async move {
			let result = if let Some(identity) = identity {
				repository
					.authorize(identity, &row, "transaction.read")
					.await
			} else {
				Ok(())
			};
			(row, result)
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

pub async fn details(
	repository: &dyn ManagementRepository,
	identity: Option<&ExecutionPrincipal>,
	id: Uuid,
) -> Result<Details> {
	if let Some(identity) = identity {
		repository.require_owner(identity, id).await?;
	}
	let transaction = repository.status(id).await?;
	if let Some(identity) = identity {
		repository
			.authorize(identity, &transaction, "transaction.read")
			.await?;
	}
	let (participants, history) = repository.history(id).await?;
	Ok(Details {
		transaction,
		participants,
		history,
	})
}

pub async fn abort(
	repository: &dyn ManagementRepository,
	identity: Option<&ExecutionPrincipal>,
	id: Uuid,
) -> Result<Status> {
	if let Some(identity) = identity {
		repository.require_owner(identity, id).await?;
		let state = repository.status(id).await?;
		repository
			.authorize(identity, &state, "transaction.abort")
			.await?;
		repository.status(id).await
	} else {
		repository.abort_operator(id).await
	}
}

pub async fn participants(repository: &dyn ManagementRepository) -> Result<Vec<LocalStatus>> {
	repository.participants().await
}

pub async fn trust_list(repository: &dyn ManagementRepository) -> Result<Vec<TrustChange>> {
	let rows = repository.trusts().await?;
	let mut result = Vec::with_capacity(rows.len());
	for trust in rows {
		let pending_transactions = if trust.enabled {
			Vec::new()
		} else {
			repository.pending_peer(&trust.node_id).await?
		};
		result.push(TrustChange {
			trust,
			pending_transactions,
		});
	}
	Ok(result)
}

pub async fn trust(repository: &dyn ManagementRepository, input: Trust) -> Result<TrustChange> {
	if input.enabled {
		repository.ensure_peer(&input.node_id).await?;
	}
	let mut scope = repository.trust_scope().await?;
	let trust = scope.set(input).await?;
	let pending_transactions = if trust.enabled {
		Vec::new()
	} else {
		scope.pending_peer(&trust.node_id).await?
	};
	Ok(TrustChange {
		trust,
		pending_transactions,
	})
}

pub async fn restore_peer(
	repository: &dyn ManagementRepository,
	node: &str,
	reference: &str,
) -> Result<Peer> {
	let credential = repository.credential(reference)?;
	repository.restore_peer(node, reference, &credential).await
}

pub async fn decision(
	repository: &dyn ManagementRepository,
	caller: Option<&str>,
	id: Uuid,
) -> Result<Status> {
	let state = repository.status(id).await?;
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	if !manifest
		.participants
		.iter()
		.any(|participant| Some(participant.node_id.as_str()) == caller)
	{
		return Err(Error::Forbidden);
	}
	Ok(state)
}

#[cfg(test)]
mod tests;

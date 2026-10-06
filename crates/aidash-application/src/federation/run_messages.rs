//! Home reservations, executor-ledger admission and replay keep their durable ordering.
use crate::{Error, Result, ports::federation::run_messages::RunMessages};
use aidash_domain::{Message, run_input::remote};

pub async fn admit(
	scope: &dyn RunMessages,
	sender: &str,
	content: &str,
	key: &str,
	limit: usize,
) -> Result<()> {
	if let Some(input) = scope
		.inputs()
		.await?
		.into_iter()
		.find(|input| input.idempotency_key == key)
	{
		if input.content != content {
			return Err(Error::Conflict("run message idempotency key reused".into()));
		}
		if let Some(message) = input.message_id
			&& scope.has_media(message).await?
		{
			return Err(Error::Conflict(
				"run message idempotency key reused with media".into(),
			));
		}
		if !scope.local() && !recover_home(scope, key, content).await? {
			return Err(Error::Conflict(
				"remote home cannot persist run message admission".into(),
			));
		}
		return Ok(());
	}
	let history = if scope.local() {
		vec![]
	} else {
		historical_batch(scope).await?
	};
	let reserved = if scope.local() {
		false
	} else if reserve(scope, key, content).await? {
		true
	} else {
		return Err(Error::Conflict(
			"remote home cannot atomically reserve run messages".into(),
		));
	};
	let admission = if reserved {
		scope
			.import_and_accept(&history, sender, content, key, limit)
			.await
	} else {
		scope.accept(sender, content, key, limit).await
	};
	if let Err(error) = admission {
		if matches!(error, Error::Conflict(_)) && recover_historical(scope, key, content).await? {
			if reserved {
				commit(scope, key, content).await?;
			}
			return Ok(());
		}
		if !scope.local() {
			match scope.sequence(key, content).await {
				Ok(_) => {
					commit(scope, key, content).await?;
					return Ok(());
				}
				Err(Error::Conflict(_)) => release(scope, &[key.to_owned()]).await?,
				Err(error) => return Err(error),
			}
		}
		return Err(error);
	}
	if reserved {
		commit(scope, key, content).await?;
	}
	Ok(())
}

pub async fn reserve(scope: &dyn RunMessages, key: &str, content: &str) -> Result<bool> {
	if scope.local() {
		return Ok(true);
	}
	scope.reserve(key, content).await
}

pub async fn promote(scope: &dyn RunMessages, key: &str, content: &str) -> Result<bool> {
	if scope.local() {
		return Ok(true);
	}
	let sequence = scope.sequence(key, content).await?;
	scope.promote(key, content, sequence).await
}

pub async fn commit(scope: &dyn RunMessages, key: &str, content: &str) -> Result<()> {
	if !promote(scope, key, content).await? {
		return Err(Error::Conflict(
			"remote home cannot persist run message admission".into(),
		));
	}
	Ok(())
}

pub async fn recover_home(scope: &dyn RunMessages, key: &str, content: &str) -> Result<bool> {
	match promote(scope, key, content).await {
		Ok(supported) => Ok(supported),
		Err(Error::Conflict(_)) => {
			// Try the durable acknowledgement before reserving pre-ledger history.
			if !reserve(scope, key, content).await? {
				return Ok(false);
			}
			commit(scope, key, content).await?;
			Ok(true)
		}
		Err(error) => Err(error),
	}
}

pub async fn release(scope: &dyn RunMessages, keys: &[String]) -> Result<()> {
	if scope.local() || keys.is_empty() {
		return Ok(());
	}
	scope.release(keys).await
}

pub async fn acknowledge(scope: &dyn RunMessages, keys: &[String]) -> Result<()> {
	if scope.local() || keys.is_empty() {
		return Ok(());
	}
	scope.acknowledge(keys).await
}

pub async fn acknowledge_observed(scope: &dyn RunMessages) -> Result<()> {
	if scope.local() || scope.run().observed_input_seq == 0 {
		return Ok(());
	}
	let keys: Vec<_> = scope
		.inputs()
		.await?
		.into_iter()
		.filter(|input| input.seq <= scope.run().observed_input_seq)
		.map(|input| input.idempotency_key)
		.collect();
	for chunk in keys.chunks(100) {
		acknowledge(scope, chunk).await?;
	}
	Ok(())
}

async fn snapshot_messages(scope: &dyn RunMessages) -> Result<Vec<Message>> {
	let mut messages = Vec::new();
	let mut after = None;
	loop {
		let page = scope.snapshot_page(after).await?;
		messages.extend(
			page.items
				.into_iter()
				.map(serde_json::from_value)
				.collect::<std::result::Result<Vec<Message>, _>>()?,
		);
		let Some(next) = page.next else {
			return Ok(messages);
		};
		if after.is_some_and(|previous| next <= previous) {
			return Err(Error::External(
				"peer snapshot cursor did not advance".into(),
			));
		}
		after = Some(next);
	}
}

pub async fn record(scope: &dyn RunMessages, key: &str, content: &str) -> Result<Message> {
	if let Some(message) = scope.delivery(key, content).await? {
		return Ok(message);
	}
	scope.legacy_message(key, content).await?;
	let full_key = remote::full_key(scope.node(), scope.run().task_id, key);
	snapshot_messages(scope)
		.await?
		.into_iter()
		.find(|message| message.idempotency_key.as_deref() == Some(&full_key))
		.ok_or_else(|| Error::External("legacy peer did not expose delivered message".into()))
}

pub async fn history(scope: &dyn RunMessages) -> Result<Vec<Message>> {
	if scope.local() {
		return Ok(vec![]);
	}
	let mut messages = Vec::new();
	loop {
		let Some(page) = scope.history_page(messages.len()).await? else {
			let legacy = snapshot_messages(scope).await?;
			if legacy.len() == 100 {
				return Err(Error::External(
					"legacy peer message snapshot may omit run history".into(),
				));
			}
			messages.extend(remote::legacy_history(scope.node(), scope.run(), legacy));
			return Ok(messages);
		};
		let count = page.len();
		messages.extend(page);
		if count < 4 {
			return Ok(messages);
		}
	}
}

pub async fn historical_batch(scope: &dyn RunMessages) -> Result<Vec<(String, Message)>> {
	Ok(remote::historical_batch(
		scope.node(),
		scope.run(),
		history(scope).await?,
	)?)
}

pub async fn reconcile(scope: &dyn RunMessages) -> Result<()> {
	if scope.local() {
		return Ok(());
	}
	let limit = scope.input_limit().await?;
	let history = historical_batch(scope).await?;
	scope.import_history(&history, limit).await
}

pub async fn recover_historical(scope: &dyn RunMessages, key: &str, content: &str) -> Result<bool> {
	if scope.local() {
		return Ok(false);
	}
	reconcile(scope).await?;
	Ok(scope
		.inputs()
		.await?
		.iter()
		.any(|input| input.idempotency_key == key && input.content == content))
}

pub async fn require_terminal_safe_delivery(scope: &dyn RunMessages) -> Result<()> {
	if scope.local() {
		return Ok(());
	}
	match scope.capability().await? {
		Some(capability) if capability["protocol"].as_u64() == Some(2) => Ok(()),
		_ => Err(Error::Conflict(
			"remote home does not support fenced run-message publication and completion".into(),
		)),
	}
}

pub async fn deliver(scope: &dyn RunMessages) -> Result<()> {
	if scope.local() {
		return Ok(());
	}
	for input in scope.inputs().await? {
		if input.message_id.is_some() && input.seq <= scope.run().observed_input_seq {
			continue;
		}
		if !recover_home(scope, &input.idempotency_key, &input.content).await? {
			tracing::debug!(run_id=%scope.run().id, "older home does not support remote message reservations");
		}
		let message = record(scope, &input.idempotency_key, &input.content).await?;
		remote::validate_delivery(
			scope.node(),
			scope.run(),
			&input.idempotency_key,
			&input.content,
			&message,
		)?;
		scope.bind(&input.idempotency_key, message.id).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;

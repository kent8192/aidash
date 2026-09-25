//! Process draining and dedicated Kubernetes probes, independent of admission.
use crate::{Result, store::Store};
use axum::{Router, extract::State, http::StatusCode, routing::get};
use tokio::sync::watch;

pub async fn signal() -> std::io::Result<()> {
	#[cfg(unix)]
	{
		let mut terminate =
			tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
		tokio::select! {
			result = tokio::signal::ctrl_c() => result?,
			_ = terminate.recv() => {},
		}
		Ok(())
	}
	#[cfg(not(unix))]
	tokio::signal::ctrl_c().await
}

pub async fn stopped(receiver: &mut watch::Receiver<bool>) {
	while !*receiver.borrow_and_update() {
		if receiver.changed().await.is_err() {
			return;
		}
	}
}

#[derive(Clone)]
struct Probe {
	store: Store,
	stopping: watch::Receiver<bool>,
}

async fn ready(State(state): State<Probe>) -> StatusCode {
	if *state.stopping.borrow() {
		return StatusCode::SERVICE_UNAVAILABLE;
	}
	match tokio::time::timeout(
		std::time::Duration::from_secs(2),
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust("1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&state.store.control_pool),
	)
	.await
	{
		Ok(Ok(_)) => StatusCode::OK,
		_ => StatusCode::SERVICE_UNAVAILABLE,
	}
}

pub async fn probes(
	address: std::net::SocketAddr,
	store: Store,
	stopping: watch::Receiver<bool>,
) -> Result<()> {
	let app = Router::new()
		.route("/live", get(|| async { StatusCode::OK }))
		.route("/ready", get(ready))
		.with_state(Probe { store, stopping });
	axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	#[rstest::rstest]
	#[tokio::test]
	async fn draining_observes_already_sent_and_closed_signals() {
		let (sender, mut receiver) = tokio::sync::watch::channel(false);
		sender.send_replace(true);
		super::stopped(&mut receiver).await;
		sender.send_replace(false);
		drop(sender);
		super::stopped(&mut receiver).await;
	}
}

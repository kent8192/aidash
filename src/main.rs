use aidash::{
    Result, api, bus::EventBus, config::Config, federation::Federation, harness::Harness,
    registry::Registry, store::Store,
};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aidash=info".into()),
        )
        .init();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    if mode == "openapi" {
        println!(
            "{}",
            api::openapi()
                .to_pretty_json()
                .map_err(|e| aidash::Error::Invalid(e.to_string()))?
        );
        return Ok(());
    }
    if !matches!(mode.as_str(), "serve" | "server" | "worker" | "migrate") {
        return Err(aidash::Error::Invalid(
            "usage: aidash [serve|server|worker|migrate|openapi]".into(),
        ));
    }
    let config = Config::from_env()?;
    let store = Store::connect(&config.database_url, config.node_id.clone()).await?;
    if mode == "migrate" {
        return Ok(());
    }
    let registry = Registry::new(store.pool.clone());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let federation = Federation {
        store,
        registry,
        config: config.clone(),
        client,
        notify: Arc::new(tokio::sync::Notify::new()),
    };
    let mut background = tokio::task::JoinSet::new();
    {
        let f = federation.for_workers().await?;
        background.spawn(aidash::transactions::coordinator::run(f));
    }
    {
        let f = federation.for_workers().await?;
        background.spawn(aidash::transactions::participant::run(f));
    }
    {
        let f = federation.for_workers().await?;
        background.spawn(aidash::generation::provision::run(f));
    }
    {
        let f = federation.for_workers().await?;
        background.spawn(aidash::semantic::worker::run(f));
    }
    if mode != "worker" {
        let f = federation.clone();
        background.spawn(async move { EventBus::run(f).await });
        let f = federation.clone();
        background.spawn(async move {
            loop {
                if let Err(error) = f.retry_deliveries().await {
                    tracing::warn!(%error, "delegation retry failed");
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
    }
    if mode != "server" {
        let workers = federation.for_workers().await?;
        // Independent workers allow one agent to wait while another makes progress.
        for _ in 0..4 {
            let h = Harness {
                federation: workers.clone(),
            };
            background.spawn(async move { h.run_worker().await });
        }
    }
    if mode != "worker" {
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        tracing::info!(node=%config.node_id,listen=%config.listen,"Aidash node started");
        let server = axum::serve(listener, api::router(federation)).with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        });
        tokio::select! {
            result=server=>result?,
            result=background.join_next()=>{return Err(aidash::Error::External(format!("background service stopped: {result:?}")));}
        }
    } else {
        tracing::info!(node=%config.node_id,"Aidash worker started");
        tokio::select! {
            _=tokio::signal::ctrl_c()=>{},
            result=background.join_next()=>{return Err(aidash::Error::External(format!("worker stopped: {result:?}")));}
        }
    }
    background.abort_all();
    while background.join_next().await.is_some() {}
    Ok(())
}

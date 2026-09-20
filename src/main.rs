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
    let (shutdown, stopping) = tokio::sync::watch::channel(false);
    let mut background = tokio::task::JoinSet::new();
    let mut workers = tokio::task::JoinSet::new();
    if let Ok(address) = std::env::var("AIDASH_PROBE_LISTEN") {
        let address = address
            .parse()
            .map_err(|_| aidash::Error::Invalid("invalid AIDASH_PROBE_LISTEN".into()))?;
        background.spawn(aidash::lifecycle::probes(
            address,
            federation.store.clone(),
            stopping.clone(),
        ));
    }
    {
        let f = federation.for_recovery().await?;
        background.spawn(aidash::transactions::coordinator::run(f));
    }
    {
        let f = federation.for_recovery().await?;
        background.spawn(aidash::transactions::participant::run(f));
    }
    {
        let f = federation.for_runtime_workers().await?;
        background.spawn(aidash::generation::provision::run(f));
    }
    {
        let f = federation.for_runtime_workers().await?;
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
        let worker_federation = federation.for_runtime_workers().await?;
        // Independent workers allow one agent to wait while another makes progress.
        for _ in 0..4 {
            let h = Harness {
                federation: worker_federation.clone(),
            };
            let stopping = stopping.clone();
            workers.spawn(async move { h.run_worker_until(stopping).await });
        }
    }
    let mut http = tokio::task::JoinSet::new();
    if mode != "worker" {
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        tracing::info!(node=%config.node_id,listen=%config.listen,"Aidash node started");
        let mut stopping = stopping.clone();
        http.spawn(async move {
            axum::serve(listener, api::router(federation))
                .with_graceful_shutdown(
                    async move { aidash::lifecycle::stopped(&mut stopping).await },
                )
                .await
        });
    } else {
        tracing::info!(node=%config.node_id,"Aidash worker started");
    }
    tokio::select! {
        result=aidash::lifecycle::signal()=>result?,
        result=background.join_next()=>return Err(aidash::Error::External(format!("background service stopped: {result:?}"))),
        result=workers.join_next(), if !workers.is_empty()=>return Err(aidash::Error::External(format!("worker stopped: {result:?}"))),
        result=http.join_next(), if !http.is_empty()=>return Err(aidash::Error::External(format!("HTTP server stopped: {result:?}"))),
    }
    shutdown.send_replace(true);
    tracing::info!("draining HTTP requests and current worker steps");
    let drained = tokio::time::timeout(Duration::from_secs(20), async {
        while workers.join_next().await.is_some() {}
        while http.join_next().await.is_some() {}
    })
    .await
    .is_ok();
    if !drained {
        tracing::warn!(
            "drain deadline reached; unfinished durable work will recover after lease expiry"
        );
    }
    workers.abort_all();
    http.abort_all();
    background.abort_all();
    while workers.join_next().await.is_some() {}
    while http.join_next().await.is_some() {}
    while background.join_next().await.is_some() {}
    Ok(())
}

//! Nanosecond input must survive the canonical SQL and independent ledger boundary.
use super::*;
use chrono::{DateTime, SubsecRound, Utc};

#[rstest]
#[tokio::test]
async fn nanosecond_occurrence_is_canonical_across_admission_replay_and_restore(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	});
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let start: DateTime<Utc> = "2026-10-06T12:34:56.999999800Z".parse().unwrap();
	let end: DateTime<Utc> = "2026-10-06T12:34:57.123456789Z".parse().unwrap();
	let mut input = content("Canonical clock / 記憶の時刻精度");
	input.occurred = Some(TimeRange { start, end });
	let request = mutation(
		&bank,
		Change::Add {
			id: Uuid::now_v7(),
			content: input,
		},
	);
	let admitted = memory::mutate(&store, &Actor::Operator, request.clone())
		.await
		.unwrap();
	let unit = &admitted[0];
	assert_eq!(unit.learned_at.timestamp_subsec_nanos() % 1000, 0);
	assert_eq!(
		unit.content.occurred,
		Some(TimeRange {
			start: start.trunc_subsecs(6),
			end: end.trunc_subsecs(6),
		})
	);
	let read = memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	let persisted = memory::list(&store, &Actor::Operator, read.clone())
		.await
		.unwrap();
	let replayed = memory::mutate(&store, &Actor::Operator, request)
		.await
		.unwrap();
	for actual in [&persisted[0], &replayed[0]] {
		assert_eq!(actual.learned_at, unit.learned_at);
		assert_eq!(actual.content, unit.content);
		assert_eq!(
			recovery::digest(actual).unwrap(),
			recovery::digest(unit).unwrap()
		);
	}
	let directory = database.recovery_directory.path();
	let archive =
		aidash_server::semantic::services::memory_recovery::backup(&store, directory, bank)
			.await
			.unwrap();
	let report =
		aidash_server::semantic::services::memory_recovery::restore(&store, directory, &archive)
			.await
			.unwrap();
	assert_eq!((report.restored, report.withheld), (1, 0));
	let restored = memory::list(&store, &Actor::Operator, read).await.unwrap();
	assert_eq!(restored[0].content, unit.content);
	assert_eq!(restored[0].learned_at, unit.learned_at);
	server.abort();
}

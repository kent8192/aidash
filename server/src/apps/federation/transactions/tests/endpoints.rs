use crate::endpoint::{
	EndpointFixture, assert_json, assert_json_rejection, endpoint, subject, workspace,
};
use aidash_server::apps::federation::transactions::models::states::AtomicCoordinatorDecision;
use aidash_server::apps::federation::transactions::models::{AtomicCoordinator, AtomicHistory};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn atomic_submission_validates_the_entire_manifest_before_creating_work(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	// Arrange
	let app = endpoint.await;
	let subject = subject(&app, "alice", _credential_client_0.await).await;
	let created = workspace(&app.operator, "Atomic validation").await;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":app.runtime.config.node_id,"isolation":"serializable",
		"deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":app.runtime.config.node_id,
		"mutations":[{"kind":"workspace_state","workspace_id":created["id"],"expected_revision":0,"state":{"result":"committed"}}]}]});
	// Act / Assert
	let session = assert_json(subject.get("/api/session").await.unwrap(), 200);
	assert_eq!(
		session["access"],
		json!({"kind":"subject","tenant":"endpoint","subject":"alice"})
	);
	assert_eq!(
		assert_json(subject.get("/api/transactions").await.unwrap(), 200),
		json!([])
	);
	let admitted = assert_json(
		app.operator
			.post("/api/transactions", &manifest, "json")
			.await
			.unwrap(),
		202,
	);
	let replay = assert_json(
		app.operator
			.post("/api/transactions", &manifest, "json")
			.await
			.unwrap(),
		202,
	);
	assert_eq!(admitted, replay);
	assert_eq!(admitted["id"], json!(id));
	assert!(admitted["decision"].is_null());
	let mut changed = manifest.clone();
	changed["participants"][0]["mutations"][0]["state"] = json!({"different":true});
	assert_json(
		app.operator
			.post("/api/transactions", &changed, "json")
			.await
			.unwrap(),
		409,
	);
	let invalid_id = Uuid::new_v4();
	let mut external = manifest;
	external["id"] = json!(invalid_id);
	external["participants"][0]["mutations"]
		.as_array_mut()
		.unwrap()
		.push(json!({"kind":"external_tool","endpoint":"http://127.0.0.1:9/effect"}));
	assert_json_rejection(
		app.operator
			.post("/api/transactions", &external, "json")
			.await
			.unwrap(),
		422,
	);
	let rejected = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(invalid_id))
		.first_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert!(
		rejected.is_none(),
		"invalid manifests create no durable work"
	);
	let snapshot = assert_json(
		app.operator
			.get(&format!(
				"/api/workspaces/{}",
				created["id"].as_str().unwrap()
			))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(snapshot["workspace"]["state"], json!({}));
}

#[rstest]
#[tokio::test]
async fn submit_replay_abort_and_history_preserve_the_manifest(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Atomic mutation").await;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":app.runtime.config.node_id,"isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),"participants":[{"node_id":app.runtime.config.node_id,"mutations":[{"kind":"workspace_state","workspace_id":workspace["id"],"expected_revision":0,"state":{"committed":true}}]}]});
	let path = format!("/api/transactions/{id}");
	// Act
	let admitted = app
		.operator
		.post("/api/transactions", &manifest, "json")
		.await
		.unwrap();
	let replay = app
		.operator
		.post("/api/transactions", &manifest, "json")
		.await
		.unwrap();
	let abort = app
		.operator
		.post(&format!("{path}/abort"), &json!({}), "json")
		.await
		.unwrap();
	let repeated = app
		.operator
		.post(&format!("{path}/abort"), &json!({}), "json")
		.await
		.unwrap();
	let details = app.operator.get(&path).await.unwrap();
	// Assert
	assert_eq!(assert_json(admitted, 202)["id"], json!(id));
	assert_eq!(assert_json(replay, 202)["manifest"], manifest);
	assert_eq!(assert_json(abort, 200)["decision"], "ABORT");
	assert_eq!(assert_json(repeated, 200)["decision"], "ABORT");
	assert_eq!(
		assert_json(details, 200)["transaction"]["manifest"],
		manifest
	);
	let saved = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(saved.decision, Some(AtomicCoordinatorDecision::Abort));
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(history.len(), 2, "one admission and one durable abort");
	let snapshot = assert_json(
		app.operator
			.get(&format!(
				"/api/workspaces/{}",
				workspace["id"].as_str().unwrap()
			))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(snapshot["workspace"]["state"], json!({}));
}

#[rstest]
#[case::identical(false)]
#[case::conflicting(true)]
#[tokio::test]
async fn concurrent_admission_preserves_one_manifest_vote_and_audit(
	#[future] endpoint: EndpointFixture,
	#[case] conflicting: bool,
) {
	// Arrange
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Concurrent admission").await;
	let id = Uuid::new_v4();
	let first = json!({"id":id,"coordinator":app.runtime.config.node_id,"isolation":"serializable",
        "deadline":Utc::now()+Duration::minutes(5),
        "participants":[{"node_id":app.runtime.config.node_id,"mutations":[{"kind":"workspace_state",
            "workspace_id":workspace["id"],"expected_revision":0,"state":{"literal":"'$1 $2'"}}]}]});
	let mut second = first.clone();
	if conflicting {
		second["participants"][0]["mutations"][0]["state"] = json!({"other":true});
	}
	// Act
	let (a, b) = tokio::join!(
		app.operator.post("/api/transactions", &first, "json"),
		app.operator.post("/api/transactions", &second, "json"),
	);
	let a = a.unwrap();
	let b = b.unwrap();
	let (winner, loser) = if a.status_code() == 202 {
		(a, b)
	} else {
		(b, a)
	};
	let admitted = assert_json(winner, 202);
	let replay = assert_json(loser, if conflicting { 409 } else { 202 });
	let details = assert_json(
		app.operator
			.get(&format!("/api/transactions/{id}"))
			.await
			.unwrap(),
		200,
	);
	let listed = assert_json(app.operator.get("/api/transactions").await.unwrap(), 200);
	// Assert
	if !conflicting {
		assert_eq!(replay["manifest"], first);
	}
	assert_eq!(details["transaction"]["manifest"], admitted["manifest"]);
	assert_eq!(listed.as_array().unwrap().len(), 1);
	assert_eq!(listed[0]["id"], json!(id));
	assert_eq!(
		details["participants"],
		json!([{"node_id":app.runtime.config.node_id,"phase":"PENDING"}])
	);
	assert_eq!(details["history"].as_array().unwrap().len(), 1);
	assert_eq!(details["history"][0]["phase"], "PENDING");
	let saved = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(saved.manifest.0, admitted["manifest"]);
}

#[rstest]
#[tokio::test]
async fn operator_abort_is_durable_while_recovery_owns_the_lease(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_server::{Error, transactions::coordinator};
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
	};
	// Arrange
	let app = endpoint.await;
	let workspace = workspace(&app.operator, "Abort during recovery").await;
	let id = Uuid::new_v4();
	let manifest = json!({"id":id,"coordinator":app.runtime.config.node_id,"isolation":"serializable",
        "deadline":Utc::now()+Duration::minutes(5),
        "participants":[{"node_id":app.runtime.config.node_id,"mutations":[{"kind":"workspace_state",
            "workspace_id":workspace["id"],"expected_revision":0,"state":{"never": "applied"}}]}]});
	assert_json(
		app.operator
			.post("/api/transactions", &manifest, "json")
			.await
			.unwrap(),
		202,
	);
	let mut held = app.database.connection.begin().await.unwrap();
	let key = SimpleExpr::FunctionCall(
		Alias::new("hashtextextended").into_iden(),
		vec![
			Expr::value(format!("atomic:{id}")).into(),
			Expr::value(0_i64).into(),
		],
	);
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			Alias::new("pg_advisory_xact_lock").into_iden(),
			vec![key],
		))
		.build(PostgresQueryBuilder);
	held.execute(&sql, convert_values(values)).await.unwrap();
	// Act
	let pending = coordinator::advance(&app.runtime, id).await;
	let aborted = tokio::time::timeout(
		std::time::Duration::from_secs(2),
		app.operator
			.post(&format!("/api/transactions/{id}/abort"), &json!({}), "json"),
	)
	.await
	.expect("operator abort must not wait for peer recovery I/O")
	.unwrap();
	held.commit().await.unwrap();
	let persisted = assert_json(
		app.operator
			.get(&format!("/api/transactions/{id}"))
			.await
			.unwrap(),
		200,
	);
	// Assert
	assert!(matches!(pending, Err(Error::TransactionPending)));
	assert_eq!(assert_json(aborted, 200)["decision"], "ABORT");
	assert_eq!(persisted["transaction"]["decision"], "ABORT");
	assert_eq!(persisted["history"].as_array().unwrap().len(), 2);
	assert_eq!(persisted["history"][1]["phase"], "ABORT");
}

#[rstest]
#[tokio::test]
async fn peer_trust_changes_are_persisted_once_per_node_and_audited(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_server::apps::federation::peer::models::Peer;
	use aidash_server::config::PROTOCOL_VERSION;
	// Arrange: transaction trust uses an already registered peer.
	let app = endpoint.await;
	let peer = Peer::build()
		.node_id("aidash://transaction-peer")
		.endpoint(&app.server.url)
		.credential_env("UNUSED_TRANSACTION_PEER_FIXTURE")
		.protocol_version(PROTOCOL_VERSION)
		.enabled(true)
		.finish();
	Peer::objects()
		.create_with_conn(&mut app.database.lease.handle(), &peer)
		.await
		.unwrap();
	// Act
	for enabled in [true, true, false] {
		let input = json!({"node_id":peer.node_id,"enabled":enabled});
		let saved = assert_json(
			app.operator
				.post("/api/transactions/trust", &input, "json")
				.await
				.unwrap(),
			200,
		);
		assert_eq!(
			saved,
			json!({"node_id":peer.node_id,"enabled":enabled,"pending_transactions":[]})
		);
	}
	let peers = assert_json(
		app.operator.get("/api/transactions/trust").await.unwrap(),
		200,
	);
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(Uuid::nil()))
		.order_by(&["sequence"])
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	// Assert
	assert_eq!(
		peers,
		json!([{"node_id":peer.node_id,"enabled":false,"pending_transactions":[]}])
	);
	assert_eq!(
		history
			.iter()
			.map(|row| row.phase.as_str())
			.collect::<Vec<_>>(),
		["ENABLED", "ENABLED", "DISABLED"]
	);
	assert!(history.iter().all(|row| row.detail == peer.node_id));
}

#[rstest]
#[tokio::test]
async fn participant_list_reads_persisted_orm_state(#[future] endpoint: EndpointFixture) {
	use aidash_server::apps::federation::transactions::models::{
		AtomicParticipant, states::AtomicParticipantPhase,
	};
	// Arrange
	let app = endpoint.await;
	let participant = AtomicParticipant::build()
		.coordinator(&app.runtime.config.node_id)
		.digest("fixture-manifest")
		.manifest(json!({"literal":"'$1 $2'"}).into())
		.phase(AtomicParticipantPhase::Reserved)
		.finish();
	AtomicParticipant::objects()
		.create_with_conn(&mut app.database.lease.handle(), &participant)
		.await
		.unwrap();
	// Act
	let saved = assert_json(
		app.operator
			.get("/api/transactions/participants")
			.await
			.unwrap(),
		200,
	);
	// Assert
	assert_eq!(saved.as_array().unwrap().len(), 1);
	assert_eq!(saved[0]["id"], json!(participant.id));
	assert_eq!(saved[0]["phase"], "RESERVED");
	assert_eq!(saved[0]["manifest"], participant.manifest.0);
}

#[rstest]
#[tokio::test]
async fn visibility_lock_blocks_protected_reads_but_leaves_recovery_routes_available(
	#[future] endpoint: EndpointFixture,
) {
	use reinhardt::query::{Alias, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder};
	// Arrange
	let app = endpoint.await;
	let created = workspace(&app.operator, "Visibility barrier").await;
	let path = format!("/api/workspaces/{}", created["id"].as_str().unwrap());
	let mut held = app.database.connection.begin().await.unwrap();
	let sql = Query::select()
		.column(Alias::new("singleton"))
		.from(Alias::new("atomic_gate"))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	held.fetch_one(&sql, vec![]).await.unwrap();
	// Act
	let hidden = app.operator.get(&path).await.unwrap();
	let recovery = app.operator.get("/api/transactions").await.unwrap();
	held.rollback().await.unwrap();
	let visible = app.operator.get(&path).await.unwrap();
	// Assert
	assert_eq!(hidden.header("retry-after"), Some("1"));
	assert_eq!(hidden.header("x-aidash-transaction-pending"), Some("1"));
	assert_json(hidden, 503);
	assert_eq!(assert_json(recovery, 200), json!([]));
	let visible = assert_json(visible, 200);
	assert_eq!(visible["workspace"]["id"], created["id"]);
}

#[rstest]
#[tokio::test]
async fn pending_visibility_and_commit_epoch_prevent_stale_inference(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_server::apps::federation::transactions::models::{
		AtomicGate, AtomicParticipant, states::AtomicParticipantPhase,
	};
	use aidash_server::{Error, transactions::gate::ReadLease};
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	// Arrange
	let app = endpoint.await;
	let created = workspace(&app.operator, "Pending visibility").await;
	let path = format!("/api/workspaces/{}", created["id"].as_str().unwrap());
	let mut db = app.database.lease.handle();
	let mut lease = ReadLease::begin(&app.runtime.store).await.unwrap();
	lease.suspend().await.unwrap();
	let participant = AtomicParticipant::build()
		.coordinator(&app.runtime.config.node_id)
		.digest("visibility-fixture")
		.manifest(json!({}).into())
		.phase(AtomicParticipantPhase::Reserved)
		.finish();
	AtomicParticipant::objects()
		.create_with_conn(&mut db, &participant)
		.await
		.unwrap();
	let (sql, values) = Query::update()
		.table(Alias::new("atomic_gate"))
		.value_expr(Alias::new("transaction_id"), Expr::value(participant.id))
		.and_where(Expr::col(Alias::new("singleton")).eq(reinhardt::query::Expr::value(true)))
		.build(PostgresQueryBuilder);
	app.database
		.connection
		.execute(&sql, convert_values(values))
		.await
		.unwrap();
	// Act
	let hidden = app.operator.get(&path).await.unwrap();
	let (sql, values) = Query::update()
		.table(Alias::new("atomic_gate"))
		.value_expr(Alias::new("transaction_id"), Expr::null())
		.value_expr(
			Alias::new("commit_epoch"),
			Expr::col(Alias::new("commit_epoch")).add(1_i64),
		)
		.and_where(Expr::col(Alias::new("singleton")).eq(reinhardt::query::Expr::value(true)))
		.build(PostgresQueryBuilder);
	app.database
		.connection
		.execute(&sql, convert_values(values))
		.await
		.unwrap();
	let resumed = lease.resume(&app.runtime.store).await;
	// Assert
	assert_json(hidden, 503);
	assert!(matches!(resumed, Err(Error::StaleInference)));
	let gate = AtomicGate::objects()
		.filter(AtomicGate::field_singleton().eq(true))
		.get_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(gate.commit_epoch(), 1);
	assert_eq!(gate.transaction_id(), &None);
	let visible = assert_json(app.operator.get(&path).await.unwrap(), 200);
	assert_eq!(visible["workspace"]["id"], created["id"]);
}

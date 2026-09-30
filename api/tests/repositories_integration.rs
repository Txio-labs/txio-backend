//! Repository behaviour that only a real MongoDB can show: unique indexes,
//! atomic claims and TTL-bearing documents. Runs only with
//! `TXIO_TEST_MONGO_URI` set (see workspace_collaboration.rs).

use chrono::{Duration, Utc};
use mongodb::bson::oid::ObjectId;
use mongodb::Client;
use txio_api::model::session::Session;
use txio_api::model::webhook_delivery::{WebhookDelivery, STATUS_DEAD, STATUS_DELIVERED, STATUS_IN_FLIGHT};
use txio_api::repositories::{
    idempotency_repository::{Begin, IdempotencyRepository},
    session_repository::SessionRepository,
    webhook_delivery_repository::WebhookDeliveryRepository,
};

async fn db() -> Option<mongodb::Database> {
    let uri = std::env::var("TXIO_TEST_MONGO_URI").ok()?;
    let client = Client::with_uri_str(&uri).await.unwrap();
    Some(client.database(&format!("txio_it_{}", ObjectId::new().to_hex())))
}

#[tokio::test]
async fn a_session_row_is_what_keeps_a_token_alive() {
    let Some(db) = db().await else { return };
    let repo = SessionRepository::new(&db);
    repo.ensure_indexes().await.unwrap();

    let user = ObjectId::new();
    repo.save(&Session::new(user, "jti-1".into(), "Chrome on Linux".into(), "1.2.3.4".into())).await.unwrap();
    assert!(repo.exists_by_jti("jti-1").await.unwrap());
    assert!(!repo.exists_by_jti("jti-2").await.unwrap());

    repo.delete_by_jti("jti-1").await.unwrap();
    assert!(!repo.exists_by_jti("jti-1").await.unwrap(), "logout must invalidate the token");

    repo.save(&Session::new(user, "jti-3".into(), "d".into(), "ip".into())).await.unwrap();
    repo.delete_all_by_user_id(&user).await.unwrap();
    assert!(!repo.exists_by_jti("jti-3").await.unwrap());
    db.drop(None).await.unwrap();
}

#[tokio::test]
async fn idempotency_keys_block_double_execution() {
    let Some(db) = db().await else { return };
    let repo = IdempotencyRepository::new(&db);
    repo.ensure_indices().await.unwrap();
    let (user, other) = (ObjectId::new(), ObjectId::new());

    assert!(matches!(repo.begin(user, "k", "hash-a").await.unwrap(), Begin::New));
    // A concurrent duplicate sees it in progress and does not proceed.
    assert!(matches!(repo.begin(user, "k", "hash-a").await.unwrap(), Begin::InProgress));
    assert!(matches!(repo.begin(user, "k", "hash-b").await.unwrap(), Begin::Mismatch));

    repo.complete(user, "k", "0xhash").await.unwrap();
    match repo.begin(user, "k", "hash-a").await.unwrap() {
        Begin::Replay(h) => assert_eq!(h, "0xhash"),
        _ => panic!("expected a replay of the stored result"),
    }

    // Keys are per user, and a failed request releases its key for a retry.
    assert!(matches!(repo.begin(other, "k", "hash-a").await.unwrap(), Begin::New));
    repo.abort(other, "k").await.unwrap();
    assert!(matches!(repo.begin(other, "k", "hash-a").await.unwrap(), Begin::New));
    // A completed request cannot be aborted away.
    repo.abort(user, "k").await.unwrap();
    assert!(matches!(repo.begin(user, "k", "hash-a").await.unwrap(), Begin::Replay(_)));
    db.drop(None).await.unwrap();
}

#[tokio::test]
async fn webhook_deliveries_are_claimed_once_retried_and_redelivered() {
    let Some(db) = db().await else { return };
    let repo = WebhookDeliveryRepository::new(&db);
    repo.ensure_indices().await.unwrap();
    let (sub, user) = (ObjectId::new(), ObjectId::new());

    repo.insert(&WebhookDelivery::new(sub, user, "tx.confirmed".into(), "d-1".into(), "{}".into())).await.unwrap();

    let first = repo.claim_due(Duration::seconds(60)).await.unwrap().expect("due");
    assert_eq!((first.attempts, first.status.as_str()), (1, STATUS_IN_FLIGHT));
    // Leased: a second worker gets nothing.
    assert!(repo.claim_due(Duration::seconds(60)).await.unwrap().is_none());

    // A failed attempt schedules a retry in the future: not due yet.
    let id = first.id.unwrap();
    repo.mark_retry(id, Utc::now() + Duration::seconds(30), Some(500), "HTTP 500".into()).await.unwrap();
    assert!(repo.claim_due(Duration::seconds(60)).await.unwrap().is_none());

    // Once due, it is claimed again with the attempt counter advanced.
    repo.mark_retry(id, Utc::now() - Duration::seconds(1), Some(500), "HTTP 500".into()).await.unwrap();
    let second = repo.claim_due(Duration::seconds(60)).await.unwrap().expect("due again");
    assert_eq!(second.attempts, 2);

    repo.mark_delivered(id, 200).await.unwrap();
    let listed = repo.list_for_subscription(sub, user, 10).await.unwrap();
    assert_eq!((listed[0].status.as_str(), listed[0].last_status_code), (STATUS_DELIVERED, Some(200)));
    assert!(repo.list_for_subscription(sub, ObjectId::new(), 10).await.unwrap().is_empty(), "another user's deliveries are invisible");

    // Redelivery is allowed for finished deliveries only, and only by the owner.
    assert!(repo.requeue(id, ObjectId::new()).await.is_err());
    repo.requeue(id, user).await.unwrap();
    let again = repo.claim_due(Duration::seconds(60)).await.unwrap().expect("requeued");
    assert_eq!(again.delivery_id, "d-1");
    assert_eq!(again.attempts, 1);

    repo.mark_dead(id, None, "gave up".into()).await.unwrap();
    assert_eq!(repo.list_for_subscription(sub, user, 10).await.unwrap()[0].status, STATUS_DEAD);
    db.drop(None).await.unwrap();
}

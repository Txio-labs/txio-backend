//! Access-control tests against a real MongoDB. They run only when
//! `TXIO_TEST_MONGO_URI` is set (for example a throwaway container:
//! `docker run -d -p 27099:27017 mongo:7` and
//! `TXIO_TEST_MONGO_URI=mongodb://127.0.0.1:27099`). Each run uses its own
//! database and drops it afterwards.

use mongodb::bson::oid::ObjectId;
use mongodb::Client;
use serde_json::json;
use txio_api::model::user::User;
use txio_api::model::workspace::WorkspaceType;
use txio_api::repositories::{
    collection_repository::CollectionRepository, history_repository::HistoryRepository,
    request_repository::RequestRepository, rpc_repository::RpcRepository, user_repository::UserRepository,
    workspace_member_repository::WorkspaceMemberRepository, workspace_repository::WorkspaceRepository,
};
use txio_api::services::{
    collection_service::CollectionService, email_service::EmailService, sui_service::SuiService,
    workspace_access::WorkspaceAccess, workspace_service::WorkspaceService,
};
use txio_api::utils::error::AppError;

struct World {
    db: mongodb::Database,
    workspaces: WorkspaceService,
    collections: CollectionService,
    owner: (ObjectId, String),
    editor: (ObjectId, String),
    viewer: (ObjectId, String),
    stranger: (ObjectId, String),
}

async fn user(repo: &UserRepository, email: &str) -> (ObjectId, String) {
    let saved = repo.save(&User::new(email.to_string(), "x".into())).await.unwrap();
    (saved.id.unwrap(), email.to_string())
}

async fn world() -> Option<World> {
    let uri = std::env::var("TXIO_TEST_MONGO_URI").ok()?;
    let client = Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("txio_it_{}", ObjectId::new().to_hex()));

    let users = UserRepository::new(&db);
    let workspace_repo = WorkspaceRepository::new(&db);
    let members = WorkspaceMemberRepository::new(&db);
    members.ensure_indices().await.unwrap();
    let access = WorkspaceAccess::new(workspace_repo.clone(), members.clone());
    let collection_repo = CollectionRepository::new(&db);
    let request_repo = RequestRepository::new(&db);

    let workspaces = WorkspaceService::new(
        workspace_repo,
        collection_repo.clone(),
        request_repo.clone(),
        HistoryRepository::new(&db),
        members,
        users.clone(),
        EmailService::new(String::new()), // unconfigured: invites report email_sent=false
        "https://app.test".into(),
    );
    let collections = CollectionService::new(
        collection_repo,
        request_repo,
        users.clone(),
        access,
        SuiService::new(RpcRepository::new(&db), "https://dummy.sui.io".into()),
    );

    Some(World {
        owner: user(&users, "owner@x.com").await,
        editor: user(&users, "editor@x.com").await,
        viewer: user(&users, "viewer@x.com").await,
        stranger: user(&users, "stranger@x.com").await,
        db,
        workspaces,
        collections,
    })
}

fn token_from(link: &str) -> String {
    link.rsplit("invite=").next().unwrap().to_string()
}

fn is_forbidden<T>(r: &Result<T, AppError>) -> bool {
    matches!(r, Err(AppError::Forbidden(_)))
}

#[tokio::test]
async fn invites_roles_visibility_and_revocation() {
    let Some(w) = world().await else {
        eprintln!("TXIO_TEST_MONGO_URI not set; skipping");
        return;
    };

    let ws = w.workspaces.create_workspace(w.owner.0, "Team".into(), WorkspaceType::Team).await.unwrap();
    let ws_id = ws.id.unwrap();

    // Invite an editor and a viewer.
    let editor_invite = w.workspaces.invite_member(ws_id, w.owner.0, &w.owner.1, "Editor@X.com", "editor").await.unwrap();
    let viewer_invite = w.workspaces.invite_member(ws_id, w.owner.0, &w.owner.1, "viewer@x.com", "viewer").await.unwrap();
    assert!(!editor_invite.email_sent, "no email provider configured in tests");
    let editor_token = token_from(&editor_invite.accept_link);
    let viewer_token = token_from(&viewer_invite.accept_link);

    // Only members-to-be may not act yet; nobody but the owner may invite.
    assert!(is_forbidden(&w.workspaces.invite_member(ws_id, w.stranger.0, &w.stranger.1, "z@x.com", "viewer").await));
    assert!(is_forbidden(&w.collections.get_user_collections(w.editor.0, Some(ws_id)).await), "a pending invite grants nothing");
    assert!(w.workspaces.invite_member(ws_id, w.owner.0, &w.owner.1, "z@x.com", "owner").await.is_err(), "owner cannot be granted");
    assert!(w.workspaces.invite_member(ws_id, w.owner.0, &w.owner.1, "OWNER@x.com", "editor").await.is_err(), "cannot invite the owner");

    // The preview needs only the token.
    let preview = w.workspaces.preview_invite(&editor_token).await.unwrap();
    assert_eq!((preview.workspace_name.as_str(), preview.invited_email.as_str()), ("Team", "editor@x.com"));
    assert!(w.workspaces.preview_invite("nope").await.is_err());

    // The invite works only for the invited email, and only once.
    assert!(is_forbidden(&w.workspaces.accept_invite(&editor_token, w.stranger.0, &w.stranger.1).await));
    w.workspaces.accept_invite(&editor_token, w.editor.0, "EDITOR@x.com").await.unwrap();
    assert!(w.workspaces.accept_invite(&editor_token, w.editor.0, &w.editor.1).await.is_err(), "single use");
    w.workspaces.accept_invite(&viewer_token, w.viewer.0, &w.viewer.1).await.unwrap();

    // The editor sees the workspace with their role; the stranger sees nothing.
    let mine = w.workspaces.get_user_workspaces(w.editor.0).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(json!(mine[0].role), json!("editor"));
    assert!(w.workspaces.get_user_workspaces(w.stranger.0).await.unwrap().is_empty());

    // Editors create; viewers cannot; strangers cannot even list.
    let created = w.collections.create_collection(w.editor.0, ws_id, "Smoke".into(), None).await.unwrap();
    let collection_id = created.id.unwrap();
    assert!(is_forbidden(&w.collections.create_collection(w.viewer.0, ws_id, "No".into(), None).await));
    assert!(is_forbidden(&w.collections.get_user_collections(w.stranger.0, Some(ws_id)).await));

    // Everyone in the workspace sees the collection, whoever made it.
    for member in [w.owner.0, w.editor.0, w.viewer.0] {
        let list = w.collections.get_user_collections(member, Some(ws_id)).await.unwrap();
        assert_eq!(list.len(), 1);
    }

    // Requests: editor and owner write, viewer reads, stranger nothing.
    let request = w
        .collections
        .add_request(w.editor.0, collection_id, "Balance".into(), "getBalance".into(), json!([]), "RPC".into(), Some("sui".into()), None, None, None)
        .await
        .unwrap();
    let request_id = request.id.unwrap();
    assert!(is_forbidden(&w.collections.add_request(w.viewer.0, collection_id, "x".into(), "m".into(), json!([]), "RPC".into(), None, None, None, None).await));
    assert_eq!(w.collections.get_collection_requests(collection_id, w.viewer.0).await.unwrap().len(), 1);
    assert!(is_forbidden(&w.collections.get_collection_requests(collection_id, w.stranger.0).await));
    assert!(is_forbidden(&w.collections.delete_request(request_id, w.viewer.0).await));
    w.collections.update_request(request_id, w.owner.0, Some("Renamed".into()), None, None, None, None, None, None, None, None).await.unwrap();

    // Only the creator or the owner may delete a collection.
    let owners = w.collections.create_collection(w.owner.0, ws_id, "Owner's".into(), None).await.unwrap();
    assert!(is_forbidden(&w.collections.delete_collection(owners.id.unwrap(), w.editor.0).await));
    w.collections.delete_collection(owners.id.unwrap(), w.owner.0).await.unwrap();

    // A collection with no workspace stays private to its creator.
    let legacy = txio_api::model::collection::Collection::new(w.owner.0, None, "Legacy".into(), None);
    let legacy = CollectionRepository::new(&w.db).save(&legacy).await.unwrap();
    assert!(w.collections.get_collection(legacy.id.unwrap(), w.owner.0).await.is_ok());
    assert!(is_forbidden(&w.collections.get_collection(legacy.id.unwrap(), w.editor.0).await));

    // Comments: viewers take part; strangers cannot read or write; delete is author-or-owner.
    let target = request_id.to_hex();
    let c1 = w.workspaces.add_comment(ws_id, w.viewer.0, &w.viewer.1, target.clone(), "  looks right  ".into()).await.unwrap();
    assert_eq!(c1.body, "looks right");
    assert_eq!(w.workspaces.list_comments(ws_id, w.editor.0, &target).await.unwrap().len(), 1);
    assert!(is_forbidden(&w.workspaces.list_comments(ws_id, w.stranger.0, &target).await));
    assert!(is_forbidden(&w.workspaces.add_comment(ws_id, w.stranger.0, &w.stranger.1, target.clone(), "hi".into()).await));
    let c1_id = ObjectId::parse_str(&c1.id).unwrap();
    assert!(is_forbidden(&w.workspaces.delete_comment(ws_id, w.editor.0, c1_id).await));
    w.workspaces.delete_comment(ws_id, w.owner.0, c1_id).await.unwrap();

    // Members list: the viewer does not see pending invitations.
    let extra = w.workspaces.invite_member(ws_id, w.owner.0, &w.owner.1, "later@x.com", "viewer").await.unwrap();
    assert_eq!(w.workspaces.list_members(ws_id, w.owner.0).await.unwrap().len(), 4);
    assert_eq!(w.workspaces.list_members(ws_id, w.viewer.0).await.unwrap().len(), 3);

    // Role change takes effect immediately; revocation ends access at once.
    let members = w.workspaces.list_members(ws_id, w.owner.0).await.unwrap();
    let viewer_member = ObjectId::parse_str(members.iter().find(|m| m.email == "viewer@x.com").unwrap().id.clone().unwrap()).unwrap();
    assert!(is_forbidden(&w.workspaces.set_member_role(ws_id, w.editor.0, viewer_member, "editor").await));
    w.workspaces.set_member_role(ws_id, w.owner.0, viewer_member, "editor").await.unwrap();
    assert!(w.collections.add_request(w.viewer.0, collection_id, "now ok".into(), "m".into(), json!([]), "RPC".into(), None, None, None, None).await.is_ok());

    w.workspaces.remove_member(ws_id, w.owner.0, viewer_member).await.unwrap();
    assert!(is_forbidden(&w.collections.get_user_collections(w.viewer.0, Some(ws_id)).await));
    assert!(is_forbidden(&w.collections.get_collection_requests(collection_id, w.viewer.0).await));
    assert!(w.workspaces.get_user_workspaces(w.viewer.0).await.unwrap().is_empty());
    // A revoked invitation's token is dead as well.
    w.workspaces.remove_member(ws_id, w.owner.0, ObjectId::parse_str(extra.member.id.clone().unwrap()).unwrap()).await.unwrap();
    assert!(w.workspaces.preview_invite(&token_from(&extra.accept_link)).await.is_err());

    // The owner cannot leave; a member can.
    assert!(w.workspaces.leave(ws_id, w.owner.0).await.is_err());
    w.workspaces.leave(ws_id, w.editor.0).await.unwrap();
    assert!(is_forbidden(&w.collections.get_user_collections(w.editor.0, Some(ws_id)).await));

    w.db.drop(None).await.unwrap();
}

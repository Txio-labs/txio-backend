use crate::model::workspace_member::WorkspaceRole;
use crate::model::{collection::Collection, network::Network, request::SavedRequest};
use crate::repositories::{
    collection_repository::CollectionRepository, request_repository::RequestRepository,
    user_repository::UserRepository,
};
use crate::services::workspace_access::WorkspaceAccess;
use crate::services::sui_service::SuiService;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;
use serde_json::Value;

/// Returns `true` when the character at byte position `end` in `s` is a
/// name-continuation character (`[A-Za-z0-9.-]`), meaning the regex match
/// ending there is part of a longer token and should **not** be treated as a
/// SuiNS name.
/// Ported from `cli/src/chains/sui.rs` to fix the same class of bug as issue #73.
fn is_name_continuation(s: &str, end: usize) -> bool {
    s[end..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[derive(Clone)]
pub struct CollectionService {
    collection_repo: CollectionRepository,
    request_repo: RequestRepository,
    user_repo: UserRepository,
    access: WorkspaceAccess,
    sui_service: SuiService,
}

impl CollectionService {
    pub fn new(
        collection_repo: CollectionRepository,
        request_repo: RequestRepository,
        user_repo: UserRepository,
        access: WorkspaceAccess,
        sui_service: SuiService,
    ) -> Self {
        Self {
            collection_repo,
            request_repo,
            user_repo,
            access,
            sui_service,
        }
    }

    /// Checks the caller holds at least `min` in the workspace.
    async fn require_workspace_role(
        &self,
        workspace_id: ObjectId,
        user_id: ObjectId,
        min: WorkspaceRole,
    ) -> Result<WorkspaceRole, AppError> {
        Ok(self.access.require(workspace_id, user_id, min).await?.1)
    }

    /// Loads a collection and checks the caller may act on it at level `min`.
    /// A collection in a workspace follows the workspace's roles; a legacy
    /// collection with no workspace stays private to its creator.
    async fn authorize_collection(
        &self,
        collection_id: ObjectId,
        user_id: ObjectId,
        min: WorkspaceRole,
    ) -> Result<(Collection, WorkspaceRole), AppError> {
        let collection = self.collection_repo.find_by_id(collection_id).await?;
        match collection.workspace_id {
            Some(workspace_id) => {
                let role = self.require_workspace_role(workspace_id, user_id, min).await?;
                Ok((collection, role))
            }
            None if collection.user_id == user_id => Ok((collection, WorkspaceRole::Owner)),
            None => Err(AppError::Forbidden("Not authorized to access this collection".into())),
        }
    }

    /// Parses a stored `SavedRequest.network` string (e.g. `"localnet"`) into
    /// a [`Network`], surfacing an unknown value as a client error rather
    /// than panicking or silently defaulting.
    fn resolve_network_str(net_str: &str) -> Result<Network, AppError> {
        net_str
            .parse::<Network>()
            .map_err(|e| AppError::BadRequest(format!("Invalid network: {e}")))
    }

    /// Returns `true` when `url_str` is exactly one of the hardcoded,
    /// per-network default RPC URLs (see [`Network::sui_url`]). These values
    /// are baked into the binary, not user-supplied, so allowing them here
    /// does not expand what an attacker can reach through a request's
    /// `rpc_url` field — in practice the only one the checks below would
    /// otherwise reject is `Network::Localnet`'s `http://127.0.0.1:9000`,
    /// since the other networks' defaults are already `https`.
    fn is_canonical_network_default(url_str: &str) -> bool {
        Network::ALL
            .iter()
            .any(|network| network.sui_url() == url_str)
    }

    /// The hardcoded network defaults are trusted; anything else a user
    /// supplied goes through the shared SSRF guard (`utils::url_safety`), the
    /// same one webhooks use.
    async fn validate_url(url_str: &str) -> Result<(), AppError> {
        if Self::is_canonical_network_default(url_str) {
            return Ok(());
        }
        crate::utils::url_safety::validate_https_url(url_str).await
    }

    // --- Collections ---

    pub async fn create_collection(
        &self,
        user_id: ObjectId,
        workspace_id: ObjectId,
        name: String,
        description: Option<String>,
    ) -> Result<Collection, AppError> {
        self.require_workspace_role(workspace_id, user_id, WorkspaceRole::Editor).await?;

        let new_collection = Collection::new(user_id, Some(workspace_id), name, description);
        self.collection_repo.save(&new_collection).await
    }

    pub async fn get_user_collections(
        &self,
        user_id: ObjectId,
        workspace_id: Option<ObjectId>,
    ) -> Result<Vec<Collection>, AppError> {
        if let Some(workspace_id) = workspace_id {
            // Every member sees the workspace's collections, whoever created them.
            self.require_workspace_role(workspace_id, user_id, WorkspaceRole::Viewer).await?;

            return self.collection_repo.find_all_by_workspace(workspace_id).await;
        }

        self.collection_repo.find_all_by_user(user_id).await
    }

    pub async fn get_collection(
        &self,
        collection_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<Collection, AppError> {
        Ok(self.authorize_collection(collection_id, user_id, WorkspaceRole::Viewer).await?.0)
    }

    pub async fn update_collection(
        &self,
        collection_id: ObjectId,
        user_id: ObjectId,
        name: String,
        description: Option<String>,
    ) -> Result<Collection, AppError> {
        let (mut collection, _) = self.authorize_collection(collection_id, user_id, WorkspaceRole::Editor).await?;
        collection.name = name;
        collection.description = description;
        collection.updated_at = chrono::Utc::now();
        self.collection_repo.update(&collection).await
    }

    pub async fn delete_collection(
        &self,
        collection_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(), AppError> {
        // Editors may delete collections they created; only the owner can delete others'.
        let (collection, role) = self.authorize_collection(collection_id, user_id, WorkspaceRole::Editor).await?;
        if role != WorkspaceRole::Owner && collection.user_id != user_id {
            return Err(AppError::Forbidden("Only the collection's creator or the workspace owner can delete it".into()));
        }
        // Cascade delete requests
        self.request_repo
            .delete_all_by_collection(collection_id)
            .await?;
        self.collection_repo.delete(collection_id).await?;
        Ok(())
    }

    // --- Requests ---

    #[allow(clippy::too_many_arguments)]
    pub async fn add_request(
        &self,
        user_id: ObjectId,
        collection_id: ObjectId,
        name: String,
        method: String,
        params: Value,
        request_type: String,
        chain: Option<String>,
        tx_params: Option<Value>,
        network: Option<String>,
        rpc_url: Option<String>,
    ) -> Result<SavedRequest, AppError> {
        // Creating requests needs edit rights on the collection.
        let _ = self.authorize_collection(collection_id, user_id, WorkspaceRole::Editor).await?;

        let new_req = SavedRequest::new(
            collection_id,
            user_id,
            name,
            method,
            params,
            request_type,
            chain,
            tx_params,
            network,
            rpc_url,
        );
        self.request_repo.save(&new_req).await
    }

    pub async fn get_collection_requests(
        &self,
        collection_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<Vec<SavedRequest>, AppError> {
        let _ = self.authorize_collection(collection_id, user_id, WorkspaceRole::Viewer).await?;
        self.request_repo
            .find_all_by_collection(collection_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn update_request(
        &self,
        request_id: ObjectId,
        user_id: ObjectId,
        name: Option<String>,
        method: Option<String>,
        params: Option<Value>,
        // `Some(None)` means "clear this field"; `Some(Some(v))` means "set it to v";
        // `None` means the field was omitted from the request and should be left untouched.
        request_type: Option<Option<String>>,
        chain: Option<Option<String>>,
        tx_params: Option<Option<Value>>,
        network: Option<Option<String>>,
        rpc_url: Option<Option<String>>,
        last_response: Option<Option<Value>>, // Allow manual update of response (e.g. paste from UI)
    ) -> Result<SavedRequest, AppError> {
        let mut req = self.request_repo.find_by_id(request_id).await?;
        self.authorize_collection(req.collection_id, user_id, WorkspaceRole::Editor).await?;

        if let Some(n) = name {
            req.name = n;
        }
        if let Some(m) = method {
            req.method = m;
        }
        if let Some(p) = params {
            req.params = p;
        }
        if let Some(Some(rt)) = request_type {
            req.request_type = rt;
        }
        if let Some(chain) = chain {
            req.chain = chain;
        }
        if let Some(tx_params) = tx_params {
            req.tx_params = tx_params;
        }

        if let Some(network) = network {
            req.network = network;
        }
        if let Some(rpc_url) = rpc_url {
            req.rpc_url = rpc_url;
        }
        if let Some(last_response) = last_response {
            req.last_response = last_response;
        }

        req.updated_at = chrono::Utc::now();
        self.request_repo.update(&req).await
    }

    pub async fn delete_request(
        &self,
        request_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(), AppError> {
        let req = self.request_repo.find_by_id(request_id).await?;
        self.authorize_collection(req.collection_id, user_id, WorkspaceRole::Editor).await?;
        self.request_repo.delete(request_id).await
    }

    pub async fn execute_request(
        &self,
        request_id: ObjectId,
        user_id: ObjectId,
    ) -> Result<(SavedRequest, Value), AppError> {
        let mut req = self.request_repo.find_by_id(request_id).await?;
        // Running a saved request stores its last response on the shared document.
        self.authorize_collection(req.collection_id, user_id, WorkspaceRole::Editor).await?;

        // Determine RPC URL first (needed for resolution and main call)
        let final_url = if let Some(ref url) = req.rpc_url {
            url.clone()
        } else {
            let network_enum = if let Some(ref net_str) = req.network {
                Self::resolve_network_str(net_str)?
            } else {
                let user = self.user_repo.find_by_id(&user_id).await?;
                user.network
            };
            network_enum.sui_url().to_string()
        };
        Self::validate_url(&final_url).await?;
        // 1. Resolve Parameters (SuiNS)
        let mut final_params = req.params.clone();
        if let Err((code, msg)) = self
            .resolve_suins_params(&final_url, &mut final_params)
            .await
        {
            let err_val = self.sui_service.error_response(code, &msg);
            let mut updated_req = req.clone();
            updated_req.last_response = Some(err_val.clone());
            updated_req.last_executed_at = Some(chrono::Utc::now());
            self.request_repo.update(&updated_req).await?;
            return Ok((updated_req, err_val));
        }

        // 3. Execute
        let result = self
            .sui_service
            .call_rpc_direct(&final_url, user_id, &req.method, &final_params)
            .await?;

        // 4. Update Request History
        req.last_response = Some(result.clone());
        req.last_executed_at = Some(chrono::Utc::now());
        self.request_repo.update(&req).await?;

        Ok((req, result))
    }

    async fn resolve_suins_params(
        &self,
        final_url: &str,
        final_params: &mut Value,
    ) -> Result<(), (i32, String)> {
        let suins_regex = regex::Regex::new(r"[a-zA-Z0-9-]+\.sui").unwrap();
        if let Some(arr) = final_params.as_array_mut() {
            for v in arr.iter_mut() {
                if let Some(s) = v.as_str() {
                    let mut spans: Vec<(usize, usize, String)> = Vec::new();
                    for m in suins_regex.find_iter(s) {
                        if !is_name_continuation(s, m.end()) {
                            spans.push((m.start(), m.end(), m.as_str().to_string()));
                        }
                    }

                    if !spans.is_empty() {
                        let mut name_to_addr = std::collections::HashMap::new();
                        for (_, _, name) in &spans {
                            if !name_to_addr.contains_key(name) {
                                match self
                                    .sui_service
                                    .resolve_name_service_address(final_url, name)
                                    .await
                                {
                                    Ok(addr) => {
                                        name_to_addr.insert(name.clone(), addr);
                                    }
                                    Err(e) => {
                                        return Err((
                                            -32002,
                                            format!("SuiNS Resolution Error for '{name}': {e}"),
                                        ));
                                    }
                                }
                            }
                        }

                        let original = s;
                        let mut new_string = String::with_capacity(original.len());
                        let mut last_end = 0usize;
                        for (start, end, name) in spans {
                            new_string.push_str(&original[last_end..start]);
                            if let Some(addr) = name_to_addr.get(&name) {
                                new_string.push_str(addr);
                            } else {
                                new_string.push_str(&original[start..end]);
                            }
                            last_end = end;
                        }
                        new_string.push_str(&original[last_end..]);

                        *v = Value::String(new_string);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validate_url_allowed() {
        assert!(CollectionService::validate_url("https://api.mainnet.sui.io").await.is_ok());
        assert!(CollectionService::validate_url("https://fullnode.devnet.sui.io:443/").await.is_ok());
    }

    #[tokio::test]
    async fn test_validate_url_blocked_http() {
        assert!(CollectionService::validate_url("http://api.mainnet.sui.io").await.is_err());
        assert!(CollectionService::validate_url("http://1.1.1.1").await.is_err());
    }

    #[tokio::test]
    async fn test_validate_url_blocked_localhost() {
        assert!(CollectionService::validate_url("https://localhost").await.is_err());
        assert!(CollectionService::validate_url("https://localhost:443").await.is_err());
        assert!(CollectionService::validate_url("https://127.0.0.1").await.is_err());
        assert!(CollectionService::validate_url("https://[::1]").await.is_err());
    }

    #[tokio::test]
    async fn test_validate_url_blocked_private_ip() {
        // IPv4 private ranges
        assert!(CollectionService::validate_url("https://10.0.0.1").await.is_err());
        assert!(CollectionService::validate_url("https://172.16.0.1").await.is_err());
        assert!(CollectionService::validate_url("https://192.168.1.1").await.is_err());

        // IPv6 unique local addresses (ULA)
        assert!(CollectionService::validate_url("https://[fc00::1]").await.is_err());
        assert!(CollectionService::validate_url("https://[fd00::1]").await.is_err());
    }

    #[tokio::test]
    async fn test_validate_url_blocked_link_local() {
        assert!(CollectionService::validate_url("https://169.254.169.254").await.is_err());
        assert!(CollectionService::validate_url("https://[fe80::1]").await.is_err());
    }

    #[tokio::test]
    async fn test_validate_url_allows_localnet_canonical_default() {
        // Regression test for issue #358: Network::Localnet's own default RPC
        // URL is `http://127.0.0.1:9000`, which the HTTPS-only/loopback
        // checks would otherwise reject, making Localnet execution
        // impossible even though it's a fully supported network.
        assert!(CollectionService::validate_url(Network::Localnet.sui_url()).await.is_ok());
    }

    #[tokio::test]
    async fn test_validate_url_allows_every_canonical_network_default() {
        for network in Network::ALL {
            assert!(
                CollectionService::validate_url(network.sui_url()).await.is_ok(),
                "canonical default for {network} should be allowed: {}",
                network.sui_url()
            );
        }
    }

    #[tokio::test]
    async fn test_validate_url_still_blocks_non_canonical_loopback_urls() {
        // The exception is an exact match against the fixed per-network
        // default URLs, not a blanket loopback allowance: a *different*
        // loopback URL (e.g. a different port) must still be rejected.
        assert!(CollectionService::validate_url("http://127.0.0.1:9001").await.is_err());
        assert!(CollectionService::validate_url("http://127.0.0.1:9000/evil").await.is_err());
        assert!(CollectionService::validate_url("https://127.0.0.1:9000").await.is_err());
    }

    #[test]
    fn test_resolve_network_str_accepts_known_networks() {
        assert_eq!(
            CollectionService::resolve_network_str("localnet").unwrap(),
            Network::Localnet
        );
        assert_eq!(
            CollectionService::resolve_network_str("Mainnet").unwrap(),
            Network::Mainnet
        );
    }

    #[test]
    fn test_resolve_network_str_rejects_unknown_network() {
        assert!(CollectionService::resolve_network_str("supernet").is_err());
    }

    #[tokio::test]
    async fn test_validate_url_invalid_urls() {
        assert!(CollectionService::validate_url("not_a_url").await.is_err());
        assert!(CollectionService::validate_url("https://").await.is_err());
    }

    // --- Mocking utilities for testing resolve_suins_params ---
    use mongodb::Client;

    async fn dummy_collection_service() -> CollectionService {
        let client = Client::with_uri_str("mongodb://localhost:27017")
            .await
            .expect("parsing a well-formed URI must not require a live connection");
        let db = client.database("txio_db");

        let collection_repo = CollectionRepository::new(&db);
        let request_repo = RequestRepository::new(&db);
        let user_repo = UserRepository::new(&db);
        let workspace_repo = crate::repositories::workspace_repository::WorkspaceRepository::new(&db);
        let members = crate::repositories::workspace_member_repository::WorkspaceMemberRepository::new(&db);
        let access = crate::services::workspace_access::WorkspaceAccess::new(workspace_repo, members);
        let rpc_repo = crate::repositories::rpc_repository::RpcRepository::new(&db);

        let sui_service = SuiService::new(rpc_repo, "https://dummy.sui.io".to_string());

        CollectionService::new(
            collection_repo,
            request_repo,
            user_repo,
            access,
            sui_service,
        )
    }

    #[tokio::test]
    async fn suins_standalone_name_resolves() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await.unwrap();

            let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"0x123\"}".to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let service = dummy_collection_service().await;
        let mut params = serde_json::json!(["send 5 SUI to alice.sui now"]);

        service
            .resolve_suins_params(&format!("http://{addr}"), &mut params)
            .await
            .unwrap();

        assert_eq!(params[0].as_str().unwrap(), "send 5 SUI to 0x123 now");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn suins_embedded_substring_untouched() {
        let service = dummy_collection_service().await;
        // Since there is no mock server, if it tries to resolve, it will fail and return Err.
        let mut params = serde_json::json!(["invoice.suicide"]);
        service
            .resolve_suins_params("http://127.0.0.1:1", &mut params)
            .await
            .unwrap();
        assert_eq!(params[0].as_str().unwrap(), "invoice.suicide");
    }

    #[tokio::test]
    async fn suins_embedded_in_url_untouched() {
        let service = dummy_collection_service().await;
        let mut params = serde_json::json!(["attacker.sui.evil.com"]);
        service
            .resolve_suins_params("http://127.0.0.1:1", &mut params)
            .await
            .unwrap();
        assert_eq!(params[0].as_str().unwrap(), "attacker.sui.evil.com");
    }

    #[tokio::test]
    async fn suins_mixed_case() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.contains("alice.sui"));

            let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"0x123\"}".to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let service = dummy_collection_service().await;
        let mut params =
            serde_json::json!(["alice.sui and invoice.suicide and attacker.sui.evil.com"]);

        service
            .resolve_suins_params(&format!("http://{addr}"), &mut params)
            .await
            .unwrap();

        assert_eq!(
            params[0].as_str().unwrap(),
            "0x123 and invoice.suicide and attacker.sui.evil.com"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn suins_nested_json_params_untouched() {
        let service = dummy_collection_service().await;
        // The original logic only scanned strings directly in the params array.
        // Nested objects/arrays containing strings with .sui shouldn't be matched.
        let mut params = serde_json::json!([
            { "name": "alice.sui" },
            ["bob.sui"]
        ]);

        // This will succeed instantly without hitting the (nonexistent) server
        service
            .resolve_suins_params("http://127.0.0.1:1", &mut params)
            .await
            .unwrap();

        assert_eq!(params[0]["name"].as_str().unwrap(), "alice.sui");
        assert_eq!(params[1][0].as_str().unwrap(), "bob.sui");
    }

    #[tokio::test]
    async fn suins_unregistered_resolution_errors() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await.unwrap();

            // result: null indicates no resolution found
            let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let service = dummy_collection_service().await;
        let mut params = serde_json::json!(["unknown.sui"]);

        let err = service
            .resolve_suins_params(&format!("http://{addr}"), &mut params)
            .await
            .unwrap_err();
        assert_eq!(err.0, -32002);
        assert!(err.1.contains("SuiNS Resolution Error for 'unknown.sui'"));

        server.await.unwrap();
    }
}

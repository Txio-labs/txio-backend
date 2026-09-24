use serde::Deserialize;
use serde_json::Value;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateCollectionRequest {
    #[validate(length(min = 1, message = "Workspace ID is required"))]
    pub workspace_id: String,

    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CollectionQuery {
    pub workspace_id: Option<String>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateCollectionRequest {
    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Validate)]
#[validate(schema(function = "validate_create_saved_request"))]
pub struct CreateSavedRequestRequest {
    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    // Required for an RPC request; empty for a TRANSACTION request, whose
    // target lives in tx_params instead — see validate_create_saved_request.
    #[serde(default)]
    pub method: String,

    #[serde(default = "default_params")]
    pub params: Value,

    /// "RPC" or "TRANSACTION" — mirrors the frontend's RequestType enum.
    #[serde(default = "default_request_type")]
    pub request_type: String,

    pub chain: Option<String>,
    pub tx_params: Option<Value>,

    // Optional overrides
    pub network: Option<String>,
    pub rpc_url: Option<String>,
}

fn default_request_type() -> String {
    "RPC".to_string()
}

fn default_params() -> Value {
    Value::Array(Vec::new())
}

fn validate_create_saved_request(
    req: &CreateSavedRequestRequest,
) -> Result<(), validator::ValidationError> {
    if req.request_type == "RPC" && req.method.trim().is_empty() {
        let mut err = validator::ValidationError::new("method_required");
        err.message = Some("Method cannot be empty".into());
        return Err(err);
    }
    Ok(())
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateSavedRequestRequest {
    pub name: Option<String>,
    pub method: Option<String>,
    pub params: Option<Value>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub request_type: Option<Option<String>>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub chain: Option<Option<String>>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub tx_params: Option<Option<Value>>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub network: Option<Option<String>>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub rpc_url: Option<Option<String>>,

    #[serde(default, with = "serde_with::rust::double_option")]
    pub last_response: Option<Option<Value>>,
}

// Responses could just use the Models directly since they are Serialize,
// or wrap them. For simplicity, we'll return models directly in handlers.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_update_saved_request_omitted_fields() {
        let json_data = json!({
            "name": "Updated Name"
        });
        let req: UpdateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.name, Some("Updated Name".to_string()));
        assert_eq!(req.network, None);
        assert_eq!(req.rpc_url, None);
        assert_eq!(req.last_response, None);
    }

    #[test]
    fn create_request_requires_method_when_rpc() {
        let json_data = json!({ "name": "Get chain id" });
        let req: CreateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.request_type, "RPC"); // defaulted
        assert!(req.validate().is_err());
    }

    #[test]
    fn create_request_allows_empty_method_when_transaction() {
        let json_data = json!({
            "name": "Transfer USDC",
            "request_type": "TRANSACTION",
            "chain": "evm",
            "tx_params": { "to": "0xabc", "value": "0" }
        });
        let req: CreateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.method, "");
        assert!(req.validate().is_ok());
    }

    #[test]
    fn create_request_still_accepts_a_real_rpc_method() {
        let json_data = json!({
            "name": "Get chain id",
            "method": "sui_getChainIdentifier",
            "params": []
        });
        let req: CreateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_update_saved_request_explicit_null_fields() {
        let json_data = json!({
            "network": null,
            "rpc_url": null,
            "last_response": null
        });
        let req: UpdateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.network, Some(None));
        assert_eq!(req.rpc_url, Some(None));
        assert_eq!(req.last_response, Some(None));
    }

    #[test]
    fn test_update_saved_request_explicit_value_fields() {
        let json_data = json!({
            "network": "testnet",
            "rpc_url": "https://fullnode.testnet.sui.io",
            "last_response": { "status": "ok" }
        });
        let req: UpdateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.network, Some(Some("testnet".to_string())));
        assert_eq!(
            req.rpc_url,
            Some(Some("https://fullnode.testnet.sui.io".to_string()))
        );
        assert_eq!(req.last_response, Some(Some(json!({ "status": "ok" }))));
    }

    #[test]
    fn test_update_saved_request_tx_params_round_trip() {
        let json_data = json!({
            "request_type": "TRANSACTION",
            "chain": "evm",
            "tx_params": { "to": "0xabc" }
        });
        let req: UpdateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.request_type, Some(Some("TRANSACTION".to_string())));
        assert_eq!(req.chain, Some(Some("evm".to_string())));
        assert_eq!(req.tx_params, Some(Some(json!({ "to": "0xabc" }))));
    }

    #[test]
    fn test_update_saved_request_omitted_tx_fields_are_untouched() {
        let json_data = json!({ "name": "Renamed" });
        let req: UpdateSavedRequestRequest = serde_json::from_value(json_data).unwrap();
        assert_eq!(req.request_type, None);
        assert_eq!(req.chain, None);
        assert_eq!(req.tx_params, None);
    }
}

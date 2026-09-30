//! The public API's OpenAPI 3.1 document. It is written by hand next to the
//! handlers and pinned by tests (every route is present, every scope is
//! listed, and `txio-sdk/openapi.json` must equal this output), so it cannot
//! drift quietly from the code or from the SDK.

use crate::model::api_key::API_KEY_SCOPES;
use crate::model::webhook_subscription::WEBHOOK_EVENTS;
use serde_json::{json, Value};

/// (method, path) for every route mounted under `/api/public/v1`.
/// `public_api_router` must be kept in step; `document()` documents exactly these.
pub const PUBLIC_ROUTES: &[(&str, &str)] = &[
    ("get", "/openapi.json"),
    ("get", "/history"),
    ("post", "/transactions/simulate"),
    ("post", "/transactions/execute"),
];

fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ErrorEnvelope" } } }
    })
}

pub fn document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "txio public API",
            "version": "0.1.0",
            "description": "Pre-1.0: request and response shapes can change between releases. Authenticate with `Authorization: Bearer txio_live_...`. Every failure is a JSON-RPC 2.0 error object; see the ErrorEnvelope schema for the codes."
        },
        "servers": [{ "url": "https://api.txio.xyz/api/public/v1" }],
        "security": [{ "apiKey": [] }],
        "paths": {
            "/openapi.json": {
                "get": {
                    "operationId": "getOpenApiDocument",
                    "summary": "This document",
                    "security": [],
                    "responses": { "200": { "description": "The OpenAPI document", "content": { "application/json": { "schema": { "type": "object" } } } } }
                }
            },
            "/history": {
                "get": {
                    "operationId": "getHistory",
                    "summary": "Request history for the key's owner",
                    "description": "Requires scope `history:read`.",
                    "parameters": [
                        { "name": "wallet_address", "in": "query", "schema": { "type": "string" } },
                        { "name": "wallet_family", "in": "query", "schema": { "type": "string", "enum": ["sui", "evm", "solana", "aptos", "stellar"] } },
                        { "name": "chain", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": {
                        "200": {
                            "description": "History entries, newest first",
                            "headers": rate_limit_headers(),
                            "content": { "application/json": { "schema": { "type": "array", "items": { "$ref": "#/components/schemas/HistoryEntry" } } } }
                        },
                        "401": error_response("Missing, invalid or revoked API key"),
                        "403": error_response("The key lacks the required scope"),
                        "429": error_response("Rate limit exceeded; see Retry-After")
                    }
                }
            },
            "/transactions/simulate": {
                "post": {
                    "operationId": "simulateTransaction",
                    "summary": "Dry-run a transaction (not available yet)",
                    "description": "Requires scope `transactions:simulate`. Server-side simulation is not implemented, so this always answers 501 with code -32015 instead of returning a result that looks like a check passed. Simulate with the txio app or CLI, then call execute.",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/TransactionRequest" } } } },
                    "responses": {
                        "401": error_response("Missing, invalid or revoked API key"),
                        "403": error_response("The key lacks the required scope"),
                        "429": error_response("Rate limit exceeded; see Retry-After"),
                        "501": error_response("Simulation is not available server-side")
                    }
                }
            },
            "/transactions/execute": {
                "post": {
                    "operationId": "executeTransaction",
                    "summary": "Sign with a session key and broadcast (EVM only)",
                    "description": "Requires scope `transactions:execute`. Subject to the owner's spend policy and the session key's limits. Send an `Idempotency-Key` so a retried request cannot broadcast twice.",
                    "parameters": [
                        { "name": "Idempotency-Key", "in": "header", "required": false, "schema": { "type": "string", "maxLength": 128 },
                          "description": "Reusing a key with the same body returns the first result; with a different body it is a 409 (-32016)." }
                    ],
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExecuteRequest" } } } },
                    "responses": {
                        "200": {
                            "description": "The transaction was broadcast",
                            "headers": rate_limit_headers(),
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ExecuteResult" } } }
                        },
                        "400": error_response("Malformed request, unsupported chain, or a spend limit was hit"),
                        "401": error_response("Missing, invalid or revoked API key"),
                        "403": error_response("The key lacks the required scope, or the session key does not allow this"),
                        "409": error_response("Idempotency-Key conflict"),
                        "429": error_response("Rate limit exceeded; see Retry-After")
                    }
                }
            }
        },
        "webhooks": {
            "txioEvent": {
                "post": {
                    "operationId": "receiveWebhook",
                    "summary": "An event delivered to your endpoint",
                    "description": webhook_description(),
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/WebhookEvent" } } } },
                    "responses": { "2XX": { "description": "Acknowledge with any 2xx within 10 seconds. Anything else is retried; 410 disables the subscription." } }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "apiKey": { "type": "http", "scheme": "bearer", "description": "A txio API key. Scopes: history:read, routes:read, transactions:simulate, transactions:execute. A key without a route's scope gets 403 (-32011)." }
            },
            "schemas": {
                "ErrorEnvelope": {
                    "type": "object",
                    "required": ["jsonrpc", "error", "id"],
                    "properties": {
                        "jsonrpc": { "const": "2.0" },
                        "id": { "type": "null" },
                        "error": {
                            "type": "object",
                            "required": ["code", "message", "data"],
                            "properties": {
                                "code": {
                                    "type": "integer",
                                    "description": "-32000 upstream unreachable; -32001 name resolution failed; -32002 internal; -32010 unauthorized; -32011 forbidden (missing scope); -32012 rate limited; -32013 invalid request; -32014 not found; -32015 not implemented; -32016 conflict.",
                                    "enum": [-32000, -32001, -32002, -32010, -32011, -32012, -32013, -32014, -32015, -32016]
                                },
                                "message": { "type": "string" },
                                "data": {
                                    "type": "object",
                                    "required": ["status", "requestId"],
                                    "properties": { "status": { "type": "integer" }, "requestId": { "type": "string" } }
                                }
                            }
                        }
                    }
                },
                "HistoryEntry": {
                    "type": "object",
                    "required": ["id", "name", "request_type", "network", "status", "duration_ms", "executed_at"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "request_type": { "type": "string", "enum": ["RPC", "TRANSACTION"] },
                        "chain": { "type": ["string", "null"] },
                        "network": { "type": "string" },
                        "method": { "type": ["string", "null"] },
                        "wallet_family": { "type": ["string", "null"] },
                        "wallet_address": { "type": ["string", "null"] },
                        "tx_params": {},
                        "result": {},
                        "status": { "type": "integer" },
                        "duration_ms": { "type": "integer" },
                        "executed_at": { "type": "string", "format": "date-time" }
                    }
                },
                "TransactionRequest": {
                    "type": "object",
                    "required": ["chain", "tx_params"],
                    "properties": {
                        "chain": { "type": "string", "enum": ["sui", "evm", "solana", "aptos", "stellar"] },
                        "tx_params": { "description": "Chain-native transaction parameters, the same shape the txio app builds for that chain." }
                    }
                },
                "ExecuteRequest": {
                    "type": "object",
                    "required": ["session_key_id", "chain", "tx_params"],
                    "properties": {
                        "session_key_id": { "type": "string" },
                        "chain": { "type": "string", "enum": ["evm"], "description": "Only evm is supported today." },
                        "tx_params": {
                            "type": "object",
                            "required": ["chain_id", "to"],
                            "properties": {
                                "chain_id": { "type": "integer" },
                                "to": { "type": "string" },
                                "value": { "type": "string" },
                                "data": { "type": "string" }
                            }
                        }
                    }
                },
                "ExecuteResult": {
                    "type": "object",
                    "required": ["hash"],
                    "properties": { "hash": { "type": "string" } }
                },
                "WebhookEvent": {
                    "type": "object",
                    "required": ["id", "event", "created_at", "data"],
                    "properties": {
                        "id": { "type": "string", "description": "Same as the X-Txio-Delivery header; stable across retries. Deduplicate on it." },
                        "event": { "type": "string", "enum": WEBHOOK_EVENTS },
                        "created_at": { "type": "string", "format": "date-time" },
                        "data": { "type": "object" }
                    }
                }
            }
        },
        "x-txio-scopes": API_KEY_SCOPES,
    })
}

fn rate_limit_headers() -> Value {
    json!({
        "X-RateLimit-Limit": { "schema": { "type": "integer" }, "description": "Requests per minute for this key and endpoint class" },
        "X-RateLimit-Remaining": { "schema": { "type": "integer" } },
        "X-RateLimit-Reset": { "schema": { "type": "integer" }, "description": "Seconds until the budget is full again" },
        "X-Request-Id": { "schema": { "type": "string" } }
    })
}

fn webhook_description() -> String {
    "Headers: `X-Txio-Event`, `X-Txio-Delivery` (unique per event, stable across retries), `X-Txio-Timestamp` (unix seconds) and `X-Txio-Signature`. \
     Verify like this: compute HMAC-SHA256 with your signing secret over the string `<X-Txio-Timestamp>.<raw request body>`, hex-encode it, \
     and compare with the part after `v1=` in `X-Txio-Signature` using a constant-time comparison. Reject requests whose timestamp is more than 5 minutes old. \
     Deliveries are retried up to 6 times with backoff (10s, 1m, 5m, 30m, 2h, 6h); delivery is at least once, so deduplicate on the delivery id."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_every_route_and_scope() {
        let doc = document();
        for (method, path) in PUBLIC_ROUTES {
            assert!(doc["paths"][*path][*method].is_object(), "{method} {path} is not documented");
        }
        assert_eq!(doc["paths"].as_object().unwrap().len(), PUBLIC_ROUTES.len(), "documented path with no route");
        assert_eq!(doc["x-txio-scopes"].as_array().unwrap().len(), API_KEY_SCOPES.len());
        assert_eq!(doc["openapi"], "3.1.0");
    }

    #[test]
    fn every_local_ref_resolves() {
        fn walk(v: &Value, root: &Value, seen: &mut usize) {
            match v {
                Value::Object(map) => {
                    if let Some(Value::String(r)) = map.get("$ref") {
                        let pointer = r.strip_prefix('#').expect("local ref");
                        assert!(root.pointer(pointer).is_some(), "dangling $ref {r}");
                        *seen += 1;
                    }
                    map.values().for_each(|c| walk(c, root, seen));
                }
                Value::Array(items) => items.iter().for_each(|c| walk(c, root, seen)),
                _ => {}
            }
        }
        let doc = document();
        let mut seen = 0;
        walk(&doc, &doc, &mut seen);
        assert!(seen > 5);
    }

    /// `txio-sdk/openapi.json` is the copy the SDK's types are checked against.
    /// Regenerate it with `UPDATE_OPENAPI=1 cargo test -p txio-api openapi`.
    #[test]
    fn sdk_copy_matches() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../txio-sdk/openapi.json");
        let rendered = serde_json::to_string_pretty(&document()).unwrap() + "\n";
        if std::env::var("UPDATE_OPENAPI").is_ok() {
            std::fs::write(&path, &rendered).unwrap();
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(existing) => assert_eq!(existing, rendered, "txio-sdk/openapi.json is stale; run with UPDATE_OPENAPI=1"),
            Err(_) => eprintln!("txio-sdk not found next to the backend; skipping the SDK copy check"),
        }
    }
}

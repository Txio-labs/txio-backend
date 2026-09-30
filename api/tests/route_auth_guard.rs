//! Every route handler must authenticate its caller, unless it is on the
//! short list of endpoints that are public on purpose. A new handler that
//! forgets its `Claims` (or `ApiKeyAuth`) parameter fails this test instead of
//! shipping as an unauthenticated endpoint.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// (handler module, function): reachable without a session, by design.
const PUBLIC_BY_DESIGN: &[(&str, &str)] = &[
    // Sign-up and sign-in.
    ("auth_handler", "register"),
    ("auth_handler", "login"),
    ("auth_handler", "request_otp"),
    ("auth_handler", "verify_otp"),
    ("auth_handler", "forgot_password"),
    ("auth_handler", "reset_password_with_otp"),
    // OAuth entry points and callbacks (the callback proves identity to the provider).
    ("auth_handler", "providers"),
    ("auth_handler", "google_login"),
    ("auth_handler", "google_callback"),
    ("auth_handler", "github_login"),
    ("auth_handler", "github_callback"),
    ("auth_handler", "x_login"),
    ("auth_handler", "x_callback"),
    // The invitee may not have an account yet; the unguessable token is the credential.
    ("workspace_handler", "preview_invite"),
    // Documentation.
    ("public_api_handler", "openapi_document"),
];

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The parameter list of `pub async fn <name>(...)`.
fn signature_params(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("pub async fn {name}("))?;
    let rest = &source[start..];
    let end = rest.find("\n) ->").or_else(|| rest.find(") ->"))?;
    Some(rest[..end].to_string())
}

#[test]
fn every_routed_handler_authenticates_or_is_public_by_design() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
    let mut routed: BTreeSet<(String, String)> = BTreeSet::new();

    for entry in fs::read_dir(src.join("routers")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = read(&path);
        let mut rest = text.as_str();
        while let Some(i) = rest.find("_handler::") {
            let module_start = rest[..i].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |p| p + 1);
            let module = format!("{}_handler", &rest[module_start..i]);
            let after = &rest[i + "_handler::".len()..];
            let func: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !func.is_empty() && !module.starts_with("use") {
                routed.insert((module, func));
            }
            rest = after;
        }
    }
    assert!(routed.len() > 60, "found only {} routed handlers; the scan is broken", routed.len());

    let public: BTreeSet<(String, String)> =
        PUBLIC_BY_DESIGN.iter().map(|(m, f)| (m.to_string(), f.to_string())).collect();

    let mut unauthenticated = Vec::new();
    for (module, func) in &routed {
        let source = read(&src.join("handlers").join(format!("{module}.rs")));
        let params = signature_params(&source, func)
            .unwrap_or_else(|| panic!("{module}::{func} is routed but has no `pub async fn` with a return type"));
        let authenticates = params.contains("Claims") || params.contains("ApiKeyAuth");
        if !authenticates && !public.contains(&(module.clone(), func.clone())) {
            unauthenticated.push(format!("{module}::{func}"));
        }
    }
    assert!(unauthenticated.is_empty(), "handlers with no authentication: {unauthenticated:?}");

    // A public-by-design entry that is no longer routed, or that now authenticates, is stale.
    for (module, func) in &public {
        assert!(routed.contains(&(module.clone(), func.clone())), "{module}::{func} is on the public list but not routed");
    }
}

#[test]
fn account_changing_handlers_require_a_fresh_one_time_code() {
    let source = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/handlers/auth_handler.rs"));
    for func in ["update_user_email", "delete_user"] {
        let body_start = source.find(&format!("pub async fn {func}(")).unwrap();
        let body = &source[body_start..source[body_start..].find("\n}\n").map_or(source.len(), |e| body_start + e)];
        assert!(body.contains("verify_otp"), "{func} must verify a one-time code before acting");
    }
}

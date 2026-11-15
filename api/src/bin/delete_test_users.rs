//! One-off cleanup for test accounts created by manual QA/debug sessions and
//! e2e-ish ad hoc runs — not accounts created through the automated test
//! suite's own fixtures, which clean up after themselves.
//!
//! Matches the naming conventions observed in the live users collection:
//!   - `*@txio.local`                        (e.g. droplet-test@txio.local)
//!   - `*-test-<digits>@example.com`         (e.g. curl-import-test-1790278953077@example.com)
//!   - `qa-sidebar-<digits>@example.com`     (e.g. qa-sidebar-1790280246785@example.com)
//!
//! Defaults to a dry run that only prints matches. Pass --confirm to delete.
//!
//! Usage:
//! ```text
//! cargo run -p txio-api --bin delete_test_users            # dry run
//! cargo run -p txio-api --bin delete_test_users -- --confirm
//! ```

use clap::Parser;
use dotenvy::{dotenv, from_path_override};
use mongodb::bson::doc;
use serde::Deserialize;
use std::path::PathBuf;
use txio_api::infra::db::establish_connection;
use txio_api::utils::config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "delete_test_users",
    about = "Delete users matching known test-account email patterns (*@txio.local, *-test-<timestamp>@example.com)"
)]
struct Args {
    /// Actually delete matches. Without this flag, only prints what would be deleted.
    #[arg(long)]
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct MatchedUser {
    #[serde(rename = "_id")]
    id: mongodb::bson::oid::ObjectId,
    email: String,
}

fn load_project_env() {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..4 {
        let candidate = dir.join(".env");
        if candidate.exists() {
            let _ = from_path_override(&candidate);
        }
        if !dir.pop() {
            break;
        }
    }
}

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("{}", msg.as_ref());
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    load_project_env();

    let args = Args::parse();
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => fail(format!("Config error: {e}")),
    };

    let db_client = match establish_connection(&config.mongo_uri).await {
        Ok(c) => c,
        Err(e) => fail(format!("MongoDB connection failed: {e}")),
    };
    let db = db_client
        .default_database()
        .unwrap_or_else(|| db_client.database("txio_db"));
    let collection = db.collection::<MatchedUser>("users");

    // Case-insensitive; test emails are always lowercase in practice but this
    // guards against any that slipped through unnormalized.
    let filter = doc! {
        "$or": [
            { "email": { "$regex": "@txio\\.local$", "$options": "i" } },
            { "email": { "$regex": "-test-[0-9]+@example\\.com$", "$options": "i" } },
            // qa-sidebar-<timestamp>@example.com — no "-test-" segment, same
            // throwaway-QA-run convention as the others.
            { "email": { "$regex": "^qa-sidebar-[0-9]+@example\\.com$", "$options": "i" } },
        ]
    };

    let mut cursor = match collection.find(filter.clone(), None).await {
        Ok(c) => c,
        Err(e) => fail(format!("Query failed: {e}")),
    };
    let mut matches: Vec<MatchedUser> = Vec::new();
    loop {
        match cursor.advance().await {
            Ok(true) => match cursor.deserialize_current() {
                Ok(user) => matches.push(user),
                Err(e) => fail(format!("Failed reading match: {e}")),
            },
            Ok(false) => break,
            Err(e) => fail(format!("Failed reading matches: {e}")),
        }
    }

    if matches.is_empty() {
        println!("No users match the test-account patterns.");
        return;
    }

    println!("{} user(s) match:", matches.len());
    for user in &matches {
        println!("  {}  ({})", user.email, user.id.to_hex());
    }

    if !args.confirm {
        println!("\nDry run only — nothing deleted. Re-run with --confirm to delete these {} user(s).", matches.len());
        return;
    }

    let users_collection = db.collection::<mongodb::bson::Document>("users");
    match users_collection.delete_many(filter, None).await {
        Ok(result) => println!("\nDeleted {} user(s).", result.deleted_count),
        Err(e) => fail(format!("Delete failed: {e}")),
    }
}

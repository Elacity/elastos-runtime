//! ElastOS Notepad Capsule
//!
//! Capability-aware note CRUD. Each operation:
//! 1. Requests a capability token from the runtime
//! 2. Polls until the shell grants it
//! 3. Uses the token to access the user's Documents root via localhost-provider

use serde::Deserialize;

const CAPSULE_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const NOTES_ROOT_PATH: &str = "Users/self/Documents/Notes";
const NOTES_ROOT_URI: &str = "localhost://Users/self/Documents/Notes";

#[derive(Deserialize)]
struct RequestResponse {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct StatusResponse {
    status: String,
    #[serde(default)]
    token: Option<String>,
}

fn get_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("notepad: {} not set", name);
        std::process::exit(1);
    })
}

fn note_uri(name: &str) -> String {
    format!("{}/{}", NOTES_ROOT_URI, name)
}

fn note_api_path(name: &str) -> String {
    format!("{}/{}", NOTES_ROOT_PATH, name)
}

/// Request a capability and poll until granted. Returns the signed token.
fn acquire_capability(api: &str, session_token: &str, resource: &str, action: &str) -> String {
    // Request capability
    let body = serde_json::json!({
        "resource": resource,
        "action": action,
    });

    let resp = ureq::post(&format!("{}/api/capability/request", api))
        .set("Authorization", &format!("Bearer {}", session_token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .unwrap_or_else(|e| {
            eprintln!("notepad: capability request failed: {}", e);
            std::process::exit(1);
        });

    let req_resp: RequestResponse = resp.into_json().unwrap_or_else(|e| {
        eprintln!("notepad: bad response: {}", e);
        std::process::exit(1);
    });

    // If auto-granted, return token directly
    if req_resp.status.as_deref() == Some("granted") {
        if let Some(token) = req_resp.token {
            return token;
        }
    }

    let request_id = req_resp.request_id.unwrap_or_else(|| {
        eprintln!("notepad: no request_id in response");
        std::process::exit(1);
    });

    // Poll until granted (max 5 seconds)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if std::time::Instant::now() > deadline {
            eprintln!("notepad: capability grant timed out after 5s");
            std::process::exit(1);
        }

        std::thread::sleep(std::time::Duration::from_millis(50));

        let status_resp = ureq::get(&format!("{}/api/capability/request/{}", api, request_id))
            .set("Authorization", &format!("Bearer {}", session_token))
            .call();

        if let Ok(resp) = status_resp {
            if let Ok(status) = resp.into_json::<StatusResponse>() {
                match status.status.as_str() {
                    "granted" => {
                        return status.token.unwrap_or_else(|| {
                            eprintln!("notepad: granted but no token");
                            std::process::exit(1);
                        });
                    }
                    "denied" => {
                        eprintln!("notepad: capability denied");
                        std::process::exit(1);
                    }
                    "expired" => {
                        eprintln!("notepad: capability request expired");
                        std::process::exit(1);
                    }
                    _ => {} // still pending, keep polling
                }
            }
        }
    }
}

fn main() {
    eprintln!("notepad: starting v{}", CAPSULE_VERSION);
    let api = get_env("ELASTOS_API");
    let session_token = get_env("ELASTOS_TOKEN");

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: notepad <create|read|edit|delete|list> [name] [content]");
        std::process::exit(1);
    }

    let cmd = args[1].as_str();
    match cmd {
        "create" | "edit" => {
            if args.len() < 4 {
                eprintln!("Usage: notepad {} <name> <content>", cmd);
                std::process::exit(1);
            }
            let name = &args[2];
            let content = args[3..].join(" ");
            let resource = note_uri(name);
            let token = acquire_capability(&api, &session_token, &resource, "write");

            let resp = ureq::put(&format!("{}/api/localhost/{}", api, note_api_path(name)))
                .set("Authorization", &format!("Bearer {}", session_token))
                .set("X-Capability-Token", &token)
                .set("Content-Type", "application/octet-stream")
                .send_string(&content);

            let verb = if cmd == "create" { "Created" } else { "Updated" };
            match resp {
                Ok(_) => println!("{} note '{}'", verb, name),
                Err(e) => {
                    eprintln!("notepad: write failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        "read" => {
            if args.len() < 3 {
                eprintln!("Usage: notepad read <name>");
                std::process::exit(1);
            }
            let name = &args[2];
            let resource = note_uri(name);
            let token = acquire_capability(&api, &session_token, &resource, "read");

            let resp = ureq::get(&format!("{}/api/localhost/{}", api, note_api_path(name)))
                .set("Authorization", &format!("Bearer {}", session_token))
                .set("X-Capability-Token", &token)
                .call();

            match resp {
                Ok(resp) => {
                    let body = resp.into_string().unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("notepad: read failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        "delete" => {
            if args.len() < 3 {
                eprintln!("Usage: notepad delete <name>");
                std::process::exit(1);
            }
            let name = &args[2];
            let resource = note_uri(name);
            let token = acquire_capability(&api, &session_token, &resource, "delete");

            let resp = ureq::delete(&format!("{}/api/localhost/{}", api, note_api_path(name)))
                .set("Authorization", &format!("Bearer {}", session_token))
                .set("X-Capability-Token", &token)
                .call();

            match resp {
                Ok(_) => println!("Deleted note '{}'", name),
                Err(e) => {
                    eprintln!("notepad: delete failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        "list" => {
            let resource = NOTES_ROOT_URI;
            let token = acquire_capability(&api, &session_token, resource, "read");

            let resp = ureq::get(&format!("{}/api/localhost/{}?list=true", api, NOTES_ROOT_PATH))
                .set("Authorization", &format!("Bearer {}", session_token))
                .set("X-Capability-Token", &token)
                .call();

            match resp {
                Ok(resp) => {
                    let body = resp.into_string().unwrap_or_default();
                    // Parse JSON — may be {"entries":[...]} or [...]
                    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&body) {
                        let entries = obj.get("entries").and_then(|e| e.as_array())
                            .or_else(|| obj.as_array());
                        if let Some(entries) = entries {
                            for entry in entries {
                                if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                                    println!("  {}", name);
                                }
                            }
                            if entries.is_empty() {
                                println!("  (empty)");
                            }
                        } else {
                            println!("  (empty)");
                        }
                    } else {
                        println!("{}", body);
                    }
                }
                Err(e) => {
                    eprintln!("notepad: list failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        _ => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Usage: notepad <create|read|edit|delete|list> [name] [content]");
            std::process::exit(1);
        }
    }
}

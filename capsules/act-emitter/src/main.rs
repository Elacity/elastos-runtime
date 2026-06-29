//! ElastOS spend/audit verification fixture.
//!
//! Performs exactly N metered `carrier_invoke` storage writes via the SAME shipped
//! guest carrier client (`elastos_guest::runtime::RuntimeClient`) that real capsules
//! (e.g. chat) use. Each successful write debits one unit of the capsule's spend
//! budget under the canonical `vm-{name}` key, so a serve daemon started with
//! `ELASTOS_DEFAULT_SPEND_BUDGET=N` yields `spent=N` and `budget_exhausted` on the
//! (N+1)-th act. Deterministic, counted evidence — not a hand-rolled vsock call.
//!
//! Act count is read from the launch config (`ELASTOS_COMMAND`/`ELASTOS_COMMAND_B64`
//! JSON `{"count": N}`), defaulting to 7.

use elastos_guest::runtime::RuntimeClient;
use serde_json::json;

const STORAGE_RESOURCE: &str = "localhost://Public/ActEmitter/*";
const DEFAULT_COUNT: u64 = 7;

fn act_count() -> u64 {
    let payload = std::env::var("ELASTOS_COMMAND")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("ELASTOS_COMMAND_B64")
                .ok()
                .filter(|s| !s.is_empty())
                .and_then(|b64| {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                })
        });

    payload
        .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .and_then(|v| v.get("count").and_then(|c| c.as_u64()))
        .unwrap_or(DEFAULT_COUNT)
}

fn act_uri(i: u64) -> String {
    format!("localhost://Public/ActEmitter/act-{i}.txt")
}

fn main() {
    let count = act_count();
    println!("ACT_EMITTER_START count={count}");

    let mut client = RuntimeClient::new();

    let token = match client.request_capability(STORAGE_RESOURCE, "write") {
        Ok(t) => {
            println!("ACT_EMITTER_CAP ok");
            t
        }
        Err(e) => {
            println!("ACT_EMITTER_CAP ERR {e}");
            println!("ACT_EMITTER_DONE ok=0 exhausted=0 other_err=0");
            return;
        }
    };

    let mut ok = 0u64;
    let mut exhausted = 0u64;
    let mut other_err = 0u64;

    for i in 1..=count {
        let uri = act_uri(i);
        let body = json!({
            "path": uri,
            "token": token,
            "content": format!("act {i}").into_bytes(),
            "append": false,
        });
        match client.carrier_invoke(&uri, "write", &body, &token) {
            Ok(_) => {
                ok += 1;
                println!("ACT {i} ok");
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("budget_exhausted") {
                    exhausted += 1;
                    println!("ACT {i} REFUSED budget_exhausted");
                } else {
                    other_err += 1;
                    println!("ACT {i} ERR {msg}");
                }
            }
        }
    }

    println!("ACT_EMITTER_DONE ok={ok} exhausted={exhausted} other_err={other_err}");
}

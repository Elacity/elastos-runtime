//! Thin client for a long-lived `decrypt-provider` subprocess (rail-stream +
//! rail-mint), speaking the provider's newline-delimited JSON stdio protocol.
//!
//! The provider mints its hybrid session keypair in-VM at `init` and publishes the
//! public key (the demo seals the CEK to it); the secret NEVER leaves the boundary.
//! Every `StreamSegment` request relays only CEK-free sealed material + an index and
//! gets back ONLY that segment's already-decrypted bytes.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use base64::Engine;
use serde_json::{json, Value};

pub struct DecryptProviderProc {
    child: Child,
    io: Mutex<ProcIo>,
    /// The in-VM-minted decrypt-session public key (base64) the CEK is sealed to.
    pub session_pub_b64: String,
}

struct ProcIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl DecryptProviderProc {
    /// Spawn the provider binary and run `init` with the trusted authority verifying
    /// key, returning the handle once the boundary has published its session key.
    pub fn launch(binary: &str, authority_vk_b64: &str) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn decrypt-provider ({binary}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut io = ProcIo { stdin, stdout };

        let init = json!({ "op": "init", "config": { "authority_vk_b64": authority_vk_b64 } });
        let resp = request(&mut io, &init)?;
        let data = resp
            .get("data")
            .ok_or_else(|| format!("init returned no data: {resp}"))?;
        if data.get("configured").and_then(Value::as_bool) != Some(true) {
            return Err(format!("decrypt-provider did not configure: {resp}"));
        }
        let session_pub_b64 = data
            .get("decrypt_session_public_key_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("init published no session key: {resp}"))?
            .to_string();

        Ok(Self {
            child,
            io: Mutex::new(io),
            session_pub_b64,
        })
    }

    /// Relay a `StreamSegment` and return the requested segment's decrypted bytes.
    pub fn stream_segment(
        &self,
        request_value: &Value,
        material: &Value,
        index: usize,
        now_unix: u64,
    ) -> Result<Vec<u8>, String> {
        let req = json!({
            "op": "stream_segment",
            "request": request_value,
            "material": material,
            "index": index,
            "now_unix": now_unix,
        });
        let mut io = self.io.lock().map_err(|_| "rail mutex poisoned")?;
        let resp = request(&mut io, &req)?;
        if resp.get("status").and_then(Value::as_str) != Some("ok") {
            return Err(format!("stream_segment failed: {resp}"));
        }
        let b64 = resp
            .get("data")
            .and_then(|d| d.get("segment_b64"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("stream_segment returned no segment: {resp}"))?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("segment_b64 decode: {e}"))
    }
}

impl Drop for DecryptProviderProc {
    fn drop(&mut self) {
        if let Ok(mut io) = self.io.lock() {
            let _ = writeln!(io.stdin, "{}", json!({ "op": "shutdown" }));
            let _ = io.stdin.flush();
        }
        let _ = self.child.wait();
    }
}

fn request(io: &mut ProcIo, value: &Value) -> Result<Value, String> {
    writeln!(io.stdin, "{}", value).map_err(|e| format!("write to provider: {e}"))?;
    io.stdin.flush().map_err(|e| format!("flush provider stdin: {e}"))?;
    let mut line = String::new();
    let n = io
        .stdout
        .read_line(&mut line)
        .map_err(|e| format!("read provider stdout: {e}"))?;
    if n == 0 {
        return Err("decrypt-provider closed its stdout (crashed?)".to_string());
    }
    serde_json::from_str(line.trim()).map_err(|e| format!("parse provider response: {e} ({line})"))
}

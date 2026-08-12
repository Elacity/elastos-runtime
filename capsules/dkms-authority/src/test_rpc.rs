//! ADVERSARIAL, COUNTING TEST DOUBLES for the node's outbound RPC boundary (Stage 0 of the dKMS
//! security remediation).
//!
//! Two doubles, both `#[cfg(test)]`-only so nothing here can reach a production build:
//!
//! 1. [`ScriptedRpc`] — a REAL local HTTP/JSON-RPC endpoint the node's own `reqwest` client dials.
//!    It serves an ORDERED script of outcomes (`magic`, non-magic, revert, malformed, timeout,
//!    transport error) and COUNTS every request it received, so a test can pin both the DECISION a
//!    multi-endpoint pool reaches and the WORST-CASE outbound work a single request caused. Several
//!    of these stood up side by side model an operator-configured pool of disagreeing endpoints.
//! 2. [`CountingOracle`] — a trait-level [`AccessOracle`]/[`Eip1271Caller`] double with per-address
//!    scripted answers and a call counter, for the tests that care about how many on-chain reads a
//!    single grant can amplify into (no sockets needed).
//!
//! Both are deliberately dumb: no retries, no keep-alive, no hidden state. A test that passes
//! against them passes because of the NODE's behavior, not the double's.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One scripted endpoint outcome. The six shapes the plan requires a pool to be able to mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RpcReply {
    /// `eth_call` returned the EIP-1271 magic value (right-padded to a 32-byte word).
    Eip1271Magic,
    /// `eth_call` returned a well-formed, NON-magic word (an explicit "not a valid signature").
    Eip1271NonMagic,
    /// `eth_call` returned an EVM `bool` word (the `hasAccessByContentId` answer).
    Bool(bool),
    /// The contract REVERTED (a JSON-RPC error with revert semantics).
    Revert,
    /// A 200 response whose `result` is not decodable — a lying/broken endpoint.
    Malformed,
    /// No reply until past any sane client timeout: the caller must give up on its own.
    Timeout,
    /// The connection is closed with no response at all (transport-class failure).
    TransportError,
}

/// Longer than the node client's 8s `eth_call` timeout, so [`RpcReply::Timeout`] really times the
/// caller out rather than merely being slow.
const TIMEOUT_OVERSHOOT: Duration = Duration::from_secs(9);

impl RpcReply {
    /// The HTTP body this outcome answers with, or `None` for the two no-answer outcomes.
    fn body(self) -> Option<String> {
        let word = |bytes: [u8; 32]| format!("0x{}", hex::encode(bytes));
        let result =
            match self {
                RpcReply::Eip1271Magic => {
                    let mut w = [0u8; 32];
                    w[..4].copy_from_slice(&ddrm_envelope::access::EIP1271_MAGIC_VALUE);
                    word(w)
                }
                RpcReply::Eip1271NonMagic => word([0u8; 32]),
                RpcReply::Bool(b) => {
                    let mut w = [0u8; 32];
                    w[31] = u8::from(b);
                    word(w)
                }
                RpcReply::Revert => return Some(
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":3,"message":"execution reverted"}}"#
                        .to_string(),
                ),
                RpcReply::Malformed => "0xnot-hex".to_string(),
                RpcReply::Timeout | RpcReply::TransportError => return None,
            };
        Some(format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{result}"}}"#))
    }
}

/// A local HTTP JSON-RPC endpoint serving an ordered script of [`RpcReply`]s.
///
/// The script is consumed in order across ALL requests the endpoint receives; once exhausted the
/// LAST entry repeats (so a one-entry script is a constant endpoint). Every request is counted and
/// its body retained, so a test can assert both "what did the node decide" and "how much outbound
/// work did one request cost". Each connection is served on its own thread, so two concurrent
/// clients are never serialized BY THE DOUBLE — a test that observes serialization is observing the
/// node's own locking.
pub struct ScriptedRpc {
    addr: SocketAddr,
    calls: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<AtomicBool>,
}

impl ScriptedRpc {
    /// Start an endpoint serving `script` with no artificial latency.
    pub fn start(script: &[RpcReply]) -> ScriptedRpc {
        Self::start_delayed(script, Duration::ZERO)
    }

    /// Start an endpoint that waits `delay` before answering each request — the way a test holds an
    /// in-flight RPC open long enough to observe whether a caller's lock spans it.
    pub fn start_delayed(script: &[RpcReply], delay: Duration) -> ScriptedRpc {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback test endpoint");
        let addr = listener.local_addr().expect("test endpoint address");
        let calls = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let script: Vec<RpcReply> = script.to_vec();
        assert!(
            !script.is_empty(),
            "a scripted endpoint needs at least one outcome"
        );

        let served = Arc::clone(&calls);
        let recorded = Arc::clone(&bodies);
        let stopping = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            let cursor = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming() {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let script = script.clone();
                let cursor = Arc::clone(&cursor);
                let served = Arc::clone(&served);
                let recorded = Arc::clone(&recorded);
                std::thread::spawn(move || {
                    serve_one(stream, &script, &cursor, &served, &recorded, delay);
                });
            }
        });
        ScriptedRpc {
            addr,
            calls,
            bodies,
            shutdown,
        }
    }

    /// The `http://127.0.0.1:PORT` URL to put in an RPC pool.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// How many requests this endpoint has answered (or refused to answer).
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }

    /// The raw request bodies received, oldest first.
    pub fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .expect("scripted endpoint body log")
            .clone()
    }
}

impl Drop for ScriptedRpc {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Unblock the accept loop so the server thread exits with the test.
        let _ = TcpStream::connect(self.addr);
    }
}

/// Read one HTTP request off `stream`, record it, and answer per the script.
fn serve_one(
    mut stream: TcpStream,
    script: &[RpcReply],
    cursor: &AtomicUsize,
    served: &AtomicUsize,
    recorded: &Mutex<Vec<String>>,
    delay: Duration,
) {
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 1024];
    // Headers first.
    let header_end = loop {
        match stream.read(&mut buf) {
            Ok(0) => return, // the shutdown probe (or a client that hung up)
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => return,
        }
        if let Some(i) = find_subslice(&raw, b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
    let want: usize = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while raw.len() - header_end < want {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let body = String::from_utf8_lossy(&raw[header_end..]).into_owned();

    // Count + record BEFORE answering, so a timeout/transport-error outcome still counts as work
    // the node caused this endpoint to do.
    served.fetch_add(1, Ordering::AcqRel);
    recorded
        .lock()
        .expect("scripted endpoint body log")
        .push(body);
    let i = cursor.fetch_add(1, Ordering::AcqRel).min(script.len() - 1);
    let reply = script[i];

    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
    match reply.body() {
        None => {
            if reply == RpcReply::Timeout {
                std::thread::sleep(TIMEOUT_OVERSHOOT);
            }
            // TransportError (and a timed-out client) just get the socket closed.
        }
        Some(payload) => {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A trait-level access oracle: answers `hasAccessByContentId` from a per-address script and COUNTS
/// every read. Used where the question is "how many on-chain reads can ONE grant amplify into" —
/// no sockets, so the count is exactly the node's own fan-out decision.
pub struct CountingOracle {
    /// Addresses (lowercased, `0x`-prefixed) the chain says currently hold access.
    entitled: Vec<String>,
    reads: AtomicUsize,
    eip1271_calls: AtomicUsize,
}

impl CountingOracle {
    /// An oracle that answers `true` for exactly `entitled` and `false` for everything else.
    pub fn entitled(entitled: &[&str]) -> CountingOracle {
        CountingOracle {
            entitled: entitled.iter().map(|a| a.to_ascii_lowercase()).collect(),
            reads: AtomicUsize::new(0),
            eip1271_calls: AtomicUsize::new(0),
        }
    }

    /// How many `hasAccessByContentId` reads this oracle was asked for.
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Acquire)
    }

    /// How many EIP-1271 `isValidSignature` reads this oracle was asked for.
    pub fn eip1271_calls(&self) -> usize {
        self.eip1271_calls.load(Ordering::Acquire)
    }
}

impl ddrm_envelope::access::Eip1271Caller for CountingOracle {
    fn is_valid_signature(
        &self,
        _owner: &str,
        _hash: &[u8; 32],
        _sig_hex: &str,
    ) -> Option<Vec<u8>> {
        self.eip1271_calls.fetch_add(1, Ordering::AcqRel);
        None
    }
}

impl crate::node_chain::AccessOracle for CountingOracle {
    fn has_access_by_content_id(&self, holder: &str, _kid16: &[u8; 16]) -> Result<bool, String> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        Ok(self
            .entitled
            .iter()
            .any(|a| a == &holder.to_ascii_lowercase()))
    }
}

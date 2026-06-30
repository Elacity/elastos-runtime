//! W1b — per-TAP egress firewall (the default-deny containment spine).
//!
//! Turns the host-only TAP topology into ENFORCED teeth. A `guest_network`
//! microVM reaches the host runtime over a private /30 link and today has no
//! kernel rules on its TAP, so a compromised guest could probe any other host
//! port, another VM's subnet, or the host LAN. This installs a per-TAP
//! `nftables` chain pair that locks the guest to the host runtime API and
//! DROPS everything else, rate-limited-logging each drop to an NFLOG group for
//! the audit reader (W1b/C3) to turn into a signed `EgressDenied` custody event.
//!
//! Fail-closed by construction:
//! - the per-TAP chains default to `drop` for tap-origin traffic (only the host
//!   API is explicitly accepted);
//! - the in-kernel DROP NEVER depends on the audit reader being up — the `log`
//!   is a separate, rate-limited rule, so a down or flooded reader loses audit
//!   records, never containment.
//!
//! Scope (the W1b spine, decided 2026-06-29): **host-only containment**.
//! Honoring an `EgressAllowlist` so a guest can reach the public internet via
//! its OWN NIC requires net-new NAT/forwarding that does not exist today and is
//! a deliberate, separate architecture decision — it is intentionally NOT built
//! here. Today internet egress is mediated only by capability-gated host
//! providers; this firewall is the kernel backstop for that model.
//!
//! Keying (W1b/F1): the chain keys on the REAL TAP device name (`cvXXXXXXXX`,
//! from [`crate::NetworkConfig`]), NOT the capsule id. The canonical `vm-{name}`
//! is recorded later on the `EgressDenied` event for custody correlation.

use std::io::Write as _;
use std::net::Ipv4Addr;
use std::process::{Command, Stdio};

use elastos_common::{ElastosError, Result};

/// The dedicated nftables table (inet family) every per-TAP egress chain lives
/// in. Isolated from any host ruleset so teardown can never touch other rules.
pub const EGRESS_TABLE: &str = "elastos_egress";

/// The single NFLOG group every per-TAP drop-log rule feeds. The install path
/// (this crate) and the audit reader (W1b/C3) MUST agree on this number; the
/// reader demuxes per-TAP by the `elastos-egress-drop:<tap>` log prefix.
pub const EGRESS_NFLOG_GROUP: u16 = 100;

/// Max drop-log records/second per chain — bounds a flooding guest's load on
/// the audit chain. The DROP is never rate-limited, only the logging.
pub const EGRESS_LOG_RATE_PER_SEC: u32 = 10;

/// Per-TAP egress firewall for one `guest_network` microVM.
#[derive(Debug, Clone)]
pub struct EgressFirewall {
    /// The real TAP device, e.g. `cv1a2b3c4d` (NOT the capsule id).
    tap: String,
    /// The /30 host-side address — the only L3 destination the guest may reach.
    host_ip: String,
    /// The host runtime HTTP API port (the one allowed destination port).
    api_port: u16,
    /// NFLOG group the rate-limited drop `log` rules feed; the C3 reader demuxes
    /// per-TAP by the log prefix.
    nflog_group: u16,
    /// Max drop-log records/second per chain — bounds an attacker flooding the
    /// audit chain. The DROP itself is never rate-limited, only the logging.
    log_rate_per_sec: u32,
}

impl EgressFirewall {
    /// Build a per-TAP firewall, validating the TAP name and host IP up front
    /// (defense-in-depth against any injection into the generated nft script).
    pub fn new(
        tap: &str,
        host_ip: &str,
        api_port: u16,
        nflog_group: u16,
        log_rate_per_sec: u32,
    ) -> Result<Self> {
        validate_tap(tap)?;
        validate_ipv4(host_ip)?;
        Ok(Self {
            tap: tap.to_string(),
            host_ip: host_ip.to_string(),
            api_port,
            nflog_group,
            log_rate_per_sec,
        })
    }

    /// The per-TAP `input`-hook chain name (guest → host traffic).
    fn input_chain(&self) -> String {
        format!("in_{}", self.tap)
    }

    /// The per-TAP `forward`-hook chain name (guest → beyond the host).
    fn forward_chain(&self) -> String {
        format!("fw_{}", self.tap)
    }

    /// The atomic `nft -f` script that installs the per-TAP default-deny chains.
    ///
    /// Both chains begin with `iifname != <tap> return` so they are a no-op for
    /// every other interface (they coexist with the host's own ruleset and any
    /// other VM's chains). For tap-origin traffic: the `input` chain accepts
    /// only established/related + the host API, logs-then-drops the rest; the
    /// `forward` chain logs-then-drops everything (no internet egress via NIC).
    pub fn install_script(&self) -> String {
        let table = EGRESS_TABLE;
        let t = &self.tap;
        let inc = self.input_chain();
        let fwc = self.forward_chain();
        let host = &self.host_ip;
        let port = self.api_port;
        let g = self.nflog_group;
        let rate = self.log_rate_per_sec;
        format!(
            "add table inet {table}\n\
             add chain inet {table} {inc} {{ type filter hook input priority 0; policy accept; }}\n\
             flush chain inet {table} {inc}\n\
             add rule inet {table} {inc} iifname != \"{t}\" return\n\
             add rule inet {table} {inc} ct state established,related accept\n\
             add rule inet {table} {inc} ip daddr {host} tcp dport {port} accept\n\
             add rule inet {table} {inc} limit rate {rate}/second log group {g} prefix \"elastos-egress-drop:{t} \"\n\
             add rule inet {table} {inc} counter drop\n\
             add chain inet {table} {fwc} {{ type filter hook forward priority 0; policy accept; }}\n\
             flush chain inet {table} {fwc}\n\
             add rule inet {table} {fwc} iifname != \"{t}\" return\n\
             add rule inet {table} {fwc} limit rate {rate}/second log group {g} prefix \"elastos-egress-drop:{t} \"\n\
             add rule inet {table} {fwc} counter drop\n"
        )
    }

    /// The teardown commands (one per line). Run best-effort and individually so
    /// a missing chain (already torn down) never blocks removing the rest — no
    /// leaked chain and never a stale rule on a recycled TAP (BUG-2/3 discipline).
    pub fn teardown_script(&self) -> String {
        let table = EGRESS_TABLE;
        let inc = self.input_chain();
        let fwc = self.forward_chain();
        format!(
            "flush chain inet {table} {inc}\n\
             flush chain inet {table} {fwc}\n\
             delete chain inet {table} {inc}\n\
             delete chain inet {table} {fwc}\n"
        )
    }

    /// Install the chains, fail-closed: an `nft` error is propagated so the
    /// caller can refuse the launch rather than boot an un-leashed guest.
    pub fn apply(&self) -> Result<()> {
        run_nft_atomic(&self.install_script())
    }

    /// The TAP device this firewall guards (reconciliation bookkeeping key).
    pub fn tap(&self) -> &str {
        &self.tap
    }

    /// Read the TOTAL packets dropped by this TAP's chains — the ground truth for
    /// the rate-limit reconciliation (`total_dropped - per_drop_logged =
    /// suppressed`). Best-effort: `None` if nft is unavailable or the chains are
    /// already gone. Must be read BEFORE [`Self::teardown`] deletes the chains
    /// (which zeroes the counters), or the final delta is lost.
    pub fn read_drop_count(&self) -> Option<u64> {
        let mut total = 0u64;
        for chain in [self.input_chain(), self.forward_chain()] {
            let out = Command::new("nft")
                .args(["-j", "list", "chain", "inet", EGRESS_TABLE, &chain])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            total = total.saturating_add(sum_counter_packets(&out.stdout));
        }
        Some(total)
    }

    /// Remove the chains, best-effort and idempotent (errors are swallowed so a
    /// double-teardown or an already-gone chain is a no-op).
    pub fn teardown(&self) {
        for line in self.teardown_script().lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let _ = Command::new("nft")
                .args(line.split_whitespace())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Run an `nft -f -` script atomically (the whole script applies or none of it).
fn run_nft_atomic(script: &str) -> Result<()> {
    let mut child = Command::new("nft")
        .arg("-f")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ElastosError::Compute(format!("spawning nft failed: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| ElastosError::Compute("nft stdin unavailable".to_string()))?
        .write_all(script.as_bytes())
        .map_err(|e| ElastosError::Compute(format!("writing nft script failed: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| ElastosError::Compute(format!("waiting on nft failed: {e}")))?;
    if !out.status.success() {
        return Err(ElastosError::Compute(format!(
            "nft rejected the egress ruleset (status {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Sum the `packets` of every anonymous `counter` expression in an `nft -j list
/// chain` JSON document. Pure + bounds-safe (any malformed/missing field yields
/// 0, never a panic) so it is unit-testable without nft on the host.
fn sum_counter_packets(json: &[u8]) -> u64 {
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(json) else {
        return 0;
    };
    let Some(items) = doc.get("nftables").and_then(|n| n.as_array()) else {
        return 0;
    };
    let mut total = 0u64;
    for item in items {
        let Some(exprs) = item
            .get("rule")
            .and_then(|r| r.get("expr"))
            .and_then(|e| e.as_array())
        else {
            continue;
        };
        for expr in exprs {
            if let Some(packets) = expr
                .get("counter")
                .and_then(|c| c.get("packets"))
                .and_then(|p| p.as_u64())
            {
                total = total.saturating_add(packets);
            }
        }
    }
    total
}

/// A TAP name must be a Linux interface name (≤ IFNAMSIZ-1) and contain only
/// characters that are safe, unquoted, inside an nft script.
fn validate_tap(tap: &str) -> Result<()> {
    if tap.is_empty() || tap.len() > 15 {
        return Err(ElastosError::Compute(format!(
            "invalid TAP name '{tap}': must be 1..=15 chars"
        )));
    }
    if !tap
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ElastosError::Compute(format!(
            "invalid TAP name '{tap}': only [A-Za-z0-9_-] allowed"
        )));
    }
    Ok(())
}

/// The host IP must parse as an IPv4 address (it is matched as `ip daddr`).
fn validate_ipv4(ip: &str) -> Result<()> {
    ip.parse::<Ipv4Addr>()
        .map(|_| ())
        .map_err(|e| ElastosError::Compute(format!("invalid host IP '{ip}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fw() -> EgressFirewall {
        EgressFirewall::new("cv1a2b3c4d", "172.16.7.1", 8080, 100, 10).unwrap()
    }

    #[test]
    fn install_locks_input_to_the_host_api_and_default_drops() {
        let s = fw().install_script();
        // The dedicated, isolated table.
        assert!(s.contains("add table inet elastos_egress"));
        // The input chain is a base chain on the input hook, policy accept (so
        // it never hijacks the host's own input — it only acts on this tap).
        assert!(s.contains(
            "add chain inet elastos_egress in_cv1a2b3c4d { type filter hook input priority 0; policy accept; }"
        ));
        // No-op for any other interface.
        assert!(s.contains("in_cv1a2b3c4d iifname != \"cv1a2b3c4d\" return"));
        // The ONE allowed L3 destination: the host runtime API.
        assert!(s.contains("in_cv1a2b3c4d ip daddr 172.16.7.1 tcp dport 8080 accept"));
        // Everything else from the tap is dropped (fail-closed), counted for
        // the rate-limit reconciliation ground truth.
        assert!(s.trim_end().ends_with("fw_cv1a2b3c4d counter drop"));
        assert!(s.contains("in_cv1a2b3c4d counter drop"));
    }

    #[test]
    fn forward_chain_drops_all_internet_egress_no_accept() {
        let s = fw().install_script();
        // The forward chain exists and default-drops tap-origin traffic.
        assert!(s.contains(
            "add chain inet elastos_egress fw_cv1a2b3c4d { type filter hook forward priority 0; policy accept; }"
        ));
        assert!(s.contains("fw_cv1a2b3c4d iifname != \"cv1a2b3c4d\" return"));
        assert!(s.contains("fw_cv1a2b3c4d counter drop"));
        // CRUCIAL: the forward chain has NO accept RULE — no internet egress via
        // the NIC (that path is the deferred NAT decision, not this spine). The
        // base-chain `policy accept` is the default for the non-tap traffic that
        // `return`s; tap-origin traffic always hits the explicit drop, so only
        // rule verdicts are checked here.
        let fw_rules: Vec<&str> = s
            .lines()
            .filter(|l| l.starts_with("add rule inet elastos_egress fw_cv1a2b3c4d"))
            .collect();
        assert!(
            !fw_rules.iter().any(|l| l.contains("accept")),
            "forward chain must not accept any egress in the host-only spine"
        );
    }

    #[test]
    fn drop_logging_is_rate_limited_and_carries_the_tap_for_demux() {
        let s = fw().install_script();
        // Rate-limited NFLOG so a flooding guest cannot swamp the audit chain.
        assert!(s.contains(
            "limit rate 10/second log group 100 prefix \"elastos-egress-drop:cv1a2b3c4d \""
        ));
        // Both hooks log so input- and forward-direction drops are both audited.
        let log_lines = s.matches("log group 100").count();
        assert_eq!(log_lines, 2, "input and forward chains each log drops");
    }

    #[test]
    fn teardown_flushes_then_deletes_both_chains() {
        let s = fw().teardown_script();
        assert!(s.contains("flush chain inet elastos_egress in_cv1a2b3c4d"));
        assert!(s.contains("flush chain inet elastos_egress fw_cv1a2b3c4d"));
        assert!(s.contains("delete chain inet elastos_egress in_cv1a2b3c4d"));
        assert!(s.contains("delete chain inet elastos_egress fw_cv1a2b3c4d"));
        // Flush precedes delete (a base chain must be emptied before removal).
        let flush_at = s
            .find("flush chain inet elastos_egress in_cv1a2b3c4d")
            .unwrap();
        let delete_at = s
            .find("delete chain inet elastos_egress in_cv1a2b3c4d")
            .unwrap();
        assert!(flush_at < delete_at, "flush must come before delete");
    }

    #[test]
    fn distinct_taps_get_disjoint_chains_no_collision() {
        let a = EgressFirewall::new("cvaaaaaaaa", "172.16.1.1", 8080, 100, 10).unwrap();
        let b = EgressFirewall::new("cvbbbbbbbb", "172.16.2.1", 8080, 100, 10).unwrap();
        assert_ne!(a.input_chain(), b.input_chain());
        assert_ne!(a.forward_chain(), b.forward_chain());
        // a's teardown never names b's chains (no cross-TAP teardown).
        assert!(!a.teardown_script().contains("cvbbbbbbbb"));
    }

    #[test]
    fn counter_packets_are_summed_and_malformed_json_is_zero_not_panic() {
        // Two drop rules with anonymous counters (input + forward), as `nft -j
        // list chain` emits them — summed for the reconciliation ground truth.
        let json = br#"{"nftables":[
            {"rule":{"expr":[{"counter":{"packets":7,"bytes":420}},{"drop":null}]}},
            {"rule":{"expr":[{"counter":{"packets":5,"bytes":300}},{"drop":null}]}}
        ]}"#;
        assert_eq!(sum_counter_packets(json), 12);
        // Hostile / malformed inputs degrade to 0, never panic.
        assert_eq!(sum_counter_packets(b"not json at all"), 0);
        assert_eq!(sum_counter_packets(b"{}"), 0);
        assert_eq!(sum_counter_packets(b"{\"nftables\":\"wrong-type\"}"), 0);
        assert_eq!(sum_counter_packets(b""), 0);
    }

    #[test]
    fn rejects_unsafe_tap_names_and_bad_ips() {
        // Injection / oversized / empty names are refused before any nft call.
        assert!(EgressFirewall::new("cv foo; rm -rf", "172.16.1.1", 80, 1, 1).is_err());
        assert!(EgressFirewall::new("", "172.16.1.1", 80, 1, 1).is_err());
        assert!(EgressFirewall::new("cvthisistoolongforanif", "172.16.1.1", 80, 1, 1).is_err());
        assert!(EgressFirewall::new("cv1a2b3c4d", "not-an-ip", 80, 1, 1).is_err());
        assert!(EgressFirewall::new("cv1a2b3c4d", "::1", 80, 1, 1).is_err());
    }
}

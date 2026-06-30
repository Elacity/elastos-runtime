//! W1b/C3 — NFLOG drop-log ingestion (the audit half of the egress firewall).
//!
//! The per-TAP chains (see [`crate::egress_firewall`]) `log group N` every drop
//! through NFLOG, rate-limited in the kernel. This module reads that group off a
//! raw `NETLINK_NETFILTER` socket and parses each logged packet into an
//! [`EgressDrop`] (`who tried to reach what`). The server turns those into
//! signed `EgressDenied` custody events.
//!
//! Design invariants:
//! - **Enforcement is independent of this reader.** The in-kernel `drop` always
//!   happens; a down/flooded reader loses audit records, never containment.
//! - **The parser handles HOSTILE input.** A compromised guest shapes the bytes
//!   that land in the IPv4 header and (indirectly) the NFLOG frame, so every
//!   accessor is bounds-checked and returns `None`/skips on malformed or
//!   truncated input — it must never panic or take down the host process.
//! - **No audit dependency here.** The pure parser + the raw socket live in this
//!   crate; the `emit` (which needs the runtime audit log) lives in the server.

// ---------------------------------------------------------------------------
// KERNEL-ABI CONSTANTS — ground-truthed against the REAL kernel by the C4 box
// test (`elastos-server/tests/c4_egress_spine.rs`), NOT by the unit tests below.
//
// DO NOT "correct" these to match the green `--lib` tests. Those tests build AND
// parse synthetic frames with the SAME constant, so they pass for ANY value — a
// self-consistent test literally cannot catch a wrong ABI constant. Both values
// here were once wrong (`NFNL_SUBSYS_ULOG` 5→4, `NFULA_PREFIX` 2→10), passed all
// 32 parser tests, and still parsed ZERO real kernel frames. Only the live kernel
// (via NFLOG group 100 in C4) is ground truth. Cross-check against
// `<linux/netfilter/nfnetlink.h>` + `<linux/netfilter/nfnetlink_log.h>`, never
// against the unit suite. If you change one, re-run C4 (`-- --ignored`, root).
// ---------------------------------------------------------------------------

/// NFLOG message type for a logged packet: `(NFNL_SUBSYS_ULOG << 8) |
/// NFULNL_MSG_PACKET`, i.e. subsystem 4 (`NFNL_SUBSYS_ULOG`, NOT 5 = `NFNL_SUBSYS_OSF`),
/// message 0. Ground-truthed by C4 — see the ABI note above.
const NFULNL_MSG_PACKET_TYPE: u16 = 4 << 8;
/// `struct nlmsghdr` size.
const NLMSG_HDR_LEN: usize = 16;
/// `struct nfgenmsg` size (family, version, res_id).
const NFGENMSG_LEN: usize = 4;
/// NFULA_PREFIX attribute — the `log prefix` string we keyed with the TAP. The real
/// kernel stamps this as attribute type 10 (NOT 2 = `NFULA_MARK`); a wrong value here
/// makes every frame parse to `None`. Ground-truthed by C4 — see the ABI note above.
const NFULA_PREFIX: u16 = 10;
/// NFULA_PAYLOAD attribute — the raw captured packet (starts at the IP header).
const NFULA_PAYLOAD: u16 = 9;
/// The prefix our nft `log` rules stamp: `elastos-egress-drop:<tap> `.
const LOG_PREFIX_TAG: &str = "elastos-egress-drop:";

/// A single parsed egress drop: which TAP tried to reach which destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressDrop {
    /// The TAP device the drop was logged on (from the log prefix).
    pub tap: String,
    /// The blocked destination — `IP` or `IP:port`.
    pub dest: String,
    /// Transport of the blocked packet (`tcp`/`udp`/`icmp`/`ip-<n>`).
    pub proto: String,
}

/// Round a netlink attribute length up to the 4-byte alignment boundary.
fn nla_align(len: usize) -> usize {
    (len + 3) & !3
}

/// Walk the netlink attributes, returning `(prefix, payload)` slices if present.
/// Bounds-checked and panic-free on any truncated or adversarial buffer.
fn extract_attrs(attrs: &[u8]) -> (Option<&[u8]>, Option<&[u8]>) {
    let mut prefix = None;
    let mut payload = None;
    let mut off = 0usize;
    while off + 4 <= attrs.len() {
        let nla_len = u16::from_ne_bytes([attrs[off], attrs[off + 1]]) as usize;
        // Strip the nested / byte-order flags from the type.
        let nla_type = u16::from_ne_bytes([attrs[off + 2], attrs[off + 3]]) & 0x3fff;
        if nla_len < 4 || off + nla_len > attrs.len() {
            break;
        }
        let data = &attrs[off + 4..off + nla_len];
        match nla_type {
            NFULA_PREFIX => prefix = Some(data),
            NFULA_PAYLOAD => payload = Some(data),
            _ => {}
        }
        let advance = nla_align(nla_len);
        if advance == 0 {
            break;
        }
        off += advance;
    }
    (prefix, payload)
}

/// Parse one NFLOG netlink message (starting at the `nlmsghdr`) into an
/// [`EgressDrop`], or `None` for any non-packet message, a foreign log prefix,
/// or a malformed/truncated frame. Never panics.
pub fn parse_nflog_message(msg: &[u8]) -> Option<EgressDrop> {
    if msg.len() < NLMSG_HDR_LEN + NFGENMSG_LEN {
        return None;
    }
    let nlmsg_type = u16::from_ne_bytes([msg[4], msg[5]]);
    if nlmsg_type != NFULNL_MSG_PACKET_TYPE {
        return None;
    }
    let attrs = &msg[NLMSG_HDR_LEN + NFGENMSG_LEN..];
    let (prefix, payload) = extract_attrs(attrs);
    let tap = parse_tap_from_prefix(prefix?)?;
    let (dest, proto) = parse_ipv4_dest(payload?)?;
    Some(EgressDrop { tap, dest, proto })
}

/// Extract the TAP name from a `elastos-egress-drop:<tap> \0` prefix attribute.
fn parse_tap_from_prefix(prefix: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(prefix).ok()?;
    let s = s.trim_end_matches('\0').trim();
    let tap = s.strip_prefix(LOG_PREFIX_TAG)?.trim();
    if tap.is_empty() || tap.len() > 15 {
        return None;
    }
    Some(tap.to_string())
}

/// Parse an IPv4 packet's destination address + transport. Bounds-checked.
fn parse_ipv4_dest(pkt: &[u8]) -> Option<(String, String)> {
    if pkt.len() < 20 {
        return None;
    }
    if pkt[0] >> 4 != 4 {
        return None; // not IPv4 (v6 backstop is a future slice)
    }
    let ihl = (pkt[0] & 0x0f) as usize * 4;
    if ihl < 20 || ihl > pkt.len() {
        return None;
    }
    let proto_num = pkt[9];
    let dst = std::net::Ipv4Addr::new(pkt[16], pkt[17], pkt[18], pkt[19]);
    let (proto, dport) = match proto_num {
        6 => ("tcp", read_dport(pkt, ihl)),
        17 => ("udp", read_dport(pkt, ihl)),
        1 => ("icmp", None),
        n => return Some((dst.to_string(), format!("ip-{n}"))),
    };
    let dest = match dport {
        Some(p) => format!("{dst}:{p}"),
        None => dst.to_string(),
    };
    Some((dest, proto.to_string()))
}

/// Read the L4 destination port (TCP/UDP) at `ihl + 2` of the packet.
fn read_dport(pkt: &[u8], ihl: usize) -> Option<u16> {
    let off = ihl + 2;
    if off + 2 > pkt.len() {
        return None;
    }
    Some(u16::from_be_bytes([pkt[off], pkt[off + 1]]))
}

#[cfg(target_os = "linux")]
pub use linux_reader::NflogReader;

#[cfg(target_os = "linux")]
mod linux_reader {
    use super::{parse_nflog_message, EgressDrop};
    use elastos_common::{ElastosError, Result};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    const NETLINK_NETFILTER: libc::c_int = 12;
    // subsystem 4 (`NFNL_SUBSYS_ULOG`), message 1 (`NFULNL_MSG_CONFIG`). The `4` is the
    // same ground-truthed-by-C4 ABI value as `NFULNL_MSG_PACKET_TYPE` above; subsystem 5
    // (`NFNL_SUBSYS_OSF`) silently rejected our config with ERANGE. Do not "fix" to match
    // the symmetric unit tests — only the live kernel is ground truth (see the ABI note).
    const NFULNL_MSG_CONFIG_TYPE: u16 = (4 << 8) | 1;
    const NLM_F_REQUEST: u16 = 0x01;
    const NLM_F_ACK: u16 = 0x04;
    const NFULA_CFG_CMD: u16 = 1;
    const NFULA_CFG_MODE: u16 = 2;
    const NFULNL_CFG_CMD_BIND: u8 = 1;
    const NFULNL_CFG_CMD_PF_BIND: u8 = 3;
    const NFULNL_COPY_PACKET: u8 = 2;
    const AF_INET_FAMILY: u8 = 2;
    const AF_UNSPEC_FAMILY: u8 = 0;

    /// A bound NFLOG group reader. Opening or configuring it can fail (no
    /// privilege, no nfnetlink) — that is non-fatal to enforcement; the caller
    /// logs and drops the audit reader, never the firewall.
    pub struct NflogReader {
        fd: OwnedFd,
        buf: Vec<u8>,
    }

    impl NflogReader {
        /// Open a `NETLINK_NETFILTER` socket and bind the given NFLOG group.
        pub fn bind(group: u16) -> Result<Self> {
            let raw = unsafe {
                libc::socket(
                    libc::AF_NETLINK,
                    libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                    NETLINK_NETFILTER,
                )
            };
            if raw < 0 {
                return Err(ElastosError::Compute(format!(
                    "NFLOG socket() failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };

            let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
            let rc = unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    &addr as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
                )
            };
            if rc < 0 {
                return Err(ElastosError::Compute(format!(
                    "NFLOG bind() failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let reader = Self {
                fd,
                buf: vec![0u8; 64 * 1024],
            };
            // Handshake mirrors libnetfilter_log / libmnl `nf-log.c`, which is the
            // sequence the kernel actually accepts: bind this socket to the AF
            // (PF_BIND, AF_INET), bind the specific group, then request the full
            // packet copy mode. The PF_BIND is load-bearing — without it the group
            // bind is rejected and no packets are delivered (verified on the box).
            reader.send_config(AF_INET_FAMILY, 0, &cfg_cmd_attr(NFULNL_CFG_CMD_PF_BIND))?;
            reader.send_config(AF_INET_FAMILY, group, &cfg_cmd_attr(NFULNL_CFG_CMD_BIND))?;
            reader.send_config(
                AF_UNSPEC_FAMILY,
                group,
                &cfg_mode_attr(NFULNL_COPY_PACKET, 0xffff),
            )?;
            // Drain the kernel ACKs (NLM_F_ACK), failing closed if any config step
            // was rejected — a silently-misconfigured socket would lose every drop.
            reader.drain_acks()?;
            Ok(reader)
        }

        /// Send one NFULNL_MSG_CONFIG message carrying a single attribute.
        fn send_config(&self, family: u8, res_id: u16, attr: &[u8]) -> Result<()> {
            let total = 16 + 4 + attr.len();
            let mut msg = Vec::with_capacity(total);
            // nlmsghdr
            msg.extend_from_slice(&(total as u32).to_ne_bytes());
            msg.extend_from_slice(&NFULNL_MSG_CONFIG_TYPE.to_ne_bytes());
            msg.extend_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
            msg.extend_from_slice(&0u32.to_ne_bytes()); // seq
            msg.extend_from_slice(&0u32.to_ne_bytes()); // pid
                                                        // nfgenmsg: family, version=0, res_id=htons(group)
            msg.push(family);
            msg.push(0); // version
            msg.extend_from_slice(&res_id.to_be_bytes()); // res_id (network order)
            msg.extend_from_slice(attr);
            let n = unsafe {
                libc::send(
                    self.fd.as_raw_fd(),
                    msg.as_ptr() as *const libc::c_void,
                    msg.len(),
                    0,
                )
            };
            if n < 0 {
                return Err(ElastosError::Compute(format!(
                    "NFLOG config send failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(())
        }

        /// Read pending NLMSG_ERROR/ACK responses to the config handshake. Each
        /// `NLM_F_ACK` config yields one `NLMSG_ERROR` whose `error` field is `0`
        /// on success or a negative errno on rejection. Any non-zero errno is
        /// fatal to the reader (but never to enforcement). Uses a short receive
        /// timeout so a kernel that elides ACKs doesn't wedge the bind.
        fn drain_acks(&self) -> Result<()> {
            let tv = libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            };
            unsafe {
                libc::setsockopt(
                    self.fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }
            let mut buf = [0u8; 4096];
            // Three configs were sent; read until we have seen their ACKs or the
            // socket times out (best-effort — absence of an ACK is not fatal).
            for _ in 0..3 {
                let n = unsafe {
                    libc::recv(
                        self.fd.as_raw_fd(),
                        buf.as_mut_ptr() as *mut libc::c_void,
                        buf.len(),
                        0,
                    )
                };
                if n < 16 {
                    break; // timeout / short read — stop draining
                }
                let n = n as usize;
                let mut data = &buf[..n];
                while data.len() >= 16 {
                    let len = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]) as usize;
                    let msg_type = u16::from_ne_bytes([data[4], data[5]]);
                    if len < 16 || len > data.len() {
                        break;
                    }
                    // NLMSG_ERROR == 2: first 4 bytes of the body are a negative errno.
                    if msg_type == 2 && len >= 20 {
                        let err = i32::from_ne_bytes([data[16], data[17], data[18], data[19]]);
                        if err != 0 {
                            return Err(ElastosError::Compute(format!(
                                "NFLOG config rejected by kernel (errno {})",
                                -err
                            )));
                        }
                    }
                    let advance = (len + 3) & !3;
                    if advance == 0 || advance > data.len() {
                        break;
                    }
                    data = &data[advance..];
                }
            }
            // Clear the timeout so the main recv loop blocks normally.
            let tv0 = libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            };
            unsafe {
                libc::setsockopt(
                    self.fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv0 as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }
            Ok(())
        }

        /// Block for the next batch of NFLOG messages, returning the egress drops
        /// parsed from them (foreign / malformed messages are skipped).
        pub fn recv(&mut self) -> Result<Vec<EgressDrop>> {
            let n = unsafe {
                libc::recv(
                    self.fd.as_raw_fd(),
                    self.buf.as_mut_ptr() as *mut libc::c_void,
                    self.buf.len(),
                    0,
                )
            };
            if n < 0 {
                return Err(ElastosError::Compute(format!(
                    "NFLOG recv failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let mut drops = Vec::new();
            let mut data = &self.buf[..n as usize];
            // Walk the (possibly multipart) netlink stream by nlmsg_len.
            while data.len() >= 16 {
                let len = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]) as usize;
                if len < 16 || len > data.len() {
                    break;
                }
                if let Some(drop) = parse_nflog_message(&data[..len]) {
                    drops.push(drop);
                }
                let advance = (len + 3) & !3;
                if advance == 0 || advance > data.len() {
                    break;
                }
                data = &data[advance..];
            }
            Ok(drops)
        }
    }

    /// `NFULA_CFG_CMD` attribute carrying a 1-byte command. No trailing NLA pad:
    /// it is the message's only/last attribute, so `nlmsg_len` ends exactly at the
    /// 1-byte command (matching the packed-struct senders the kernel accepts).
    fn cfg_cmd_attr(command: u8) -> Vec<u8> {
        let mut a = Vec::new();
        a.extend_from_slice(&5u16.to_ne_bytes()); // nla_len = 4 + 1
        a.extend_from_slice(&NFULA_CFG_CMD.to_ne_bytes());
        a.push(command);
        a
    }

    /// `NFULA_CFG_MODE` attribute: `__be32 copy_range; u8 copy_mode; u8 _pad` (6
    /// bytes, the packed struct size). No trailing NLA pad (last attribute).
    fn cfg_mode_attr(copy_mode: u8, copy_range: u32) -> Vec<u8> {
        let mut a = Vec::new();
        a.extend_from_slice(&10u16.to_ne_bytes()); // nla_len = 4 + 6
        a.extend_from_slice(&NFULA_CFG_MODE.to_ne_bytes());
        a.extend_from_slice(&copy_range.to_be_bytes()); // network order
        a.push(copy_mode);
        a.push(0); // pad inside the struct
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal NFLOG packet message: nlmsghdr + nfgenmsg + a PREFIX attr
    /// + a PAYLOAD attr (an IPv4 header). Mirrors the kernel's wire layout.
    fn nflog_msg(prefix: &str, payload: &[u8]) -> Vec<u8> {
        fn attr(t: u16, data: &[u8]) -> Vec<u8> {
            let mut a = Vec::new();
            let len = 4 + data.len();
            a.extend_from_slice(&(len as u16).to_ne_bytes());
            a.extend_from_slice(&t.to_ne_bytes());
            a.extend_from_slice(data);
            while a.len() % 4 != 0 {
                a.push(0);
            }
            a
        }
        let mut prefix_bytes = prefix.as_bytes().to_vec();
        prefix_bytes.push(0); // NUL-terminated, as the kernel sends it
        let mut msg = Vec::new();
        let attrs = {
            let mut v = attr(NFULA_PREFIX, &prefix_bytes);
            v.extend(attr(NFULA_PAYLOAD, payload));
            v
        };
        let total = NLMSG_HDR_LEN + NFGENMSG_LEN + attrs.len();
        msg.extend_from_slice(&(total as u32).to_ne_bytes());
        msg.extend_from_slice(&NFULNL_MSG_PACKET_TYPE.to_ne_bytes());
        msg.extend_from_slice(&0u16.to_ne_bytes()); // flags
        msg.extend_from_slice(&0u32.to_ne_bytes()); // seq
        msg.extend_from_slice(&0u32.to_ne_bytes()); // pid
        msg.push(2); // nfgen_family = AF_INET
        msg.push(0);
        msg.extend_from_slice(&100u16.to_be_bytes()); // res_id
        msg.extend(attrs);
        msg
    }

    /// A 20-byte IPv4 header to `dst`, proto `p`, with an 8-byte L4 header whose
    /// first two bytes after it are the dport.
    fn ipv4(dst: [u8; 4], proto: u8, dport: u16) -> Vec<u8> {
        let mut p = vec![0u8; 28];
        p[0] = 0x45; // version 4, ihl 5 (20 bytes)
        p[9] = proto;
        p[16..20].copy_from_slice(&dst);
        // L4 header begins at offset 20; dport at +2.
        p[22..24].copy_from_slice(&dport.to_be_bytes());
        p
    }

    #[test]
    fn parses_a_well_formed_tcp_drop() {
        let msg = nflog_msg(
            "elastos-egress-drop:cv1a2b3c4d ",
            &ipv4([1, 2, 3, 4], 6, 443),
        );
        let drop = parse_nflog_message(&msg).expect("a well-formed drop parses");
        assert_eq!(drop.tap, "cv1a2b3c4d");
        assert_eq!(drop.dest, "1.2.3.4:443");
        assert_eq!(drop.proto, "tcp");
    }

    #[test]
    fn parses_udp_and_icmp_and_other_proto() {
        let udp = parse_nflog_message(&nflog_msg(
            "elastos-egress-drop:cvtap ",
            &ipv4([8, 8, 8, 8], 17, 53),
        ))
        .unwrap();
        assert_eq!(udp.dest, "8.8.8.8:53");
        assert_eq!(udp.proto, "udp");

        let icmp = parse_nflog_message(&nflog_msg(
            "elastos-egress-drop:cvtap ",
            &ipv4([9, 9, 9, 9], 1, 0),
        ))
        .unwrap();
        assert_eq!(icmp.dest, "9.9.9.9", "icmp has no port");
        assert_eq!(icmp.proto, "icmp");

        let other = parse_nflog_message(&nflog_msg(
            "elastos-egress-drop:cvtap ",
            &ipv4([10, 0, 0, 1], 47, 0),
        ))
        .unwrap();
        assert_eq!(other.proto, "ip-47");
    }

    #[test]
    fn foreign_prefix_and_wrong_message_type_are_ignored() {
        // A log from some other nft rule (not ours) is not an egress drop.
        let foreign = nflog_msg("some-other-log ", &ipv4([1, 1, 1, 1], 6, 80));
        assert!(parse_nflog_message(&foreign).is_none());

        // Wrong nlmsg_type → ignored.
        let mut wrong = nflog_msg("elastos-egress-drop:cvtap ", &ipv4([1, 1, 1, 1], 6, 80));
        wrong[4] = 0xff;
        wrong[5] = 0xff;
        assert!(parse_nflog_message(&wrong).is_none());
    }

    #[test]
    fn hostile_and_truncated_frames_never_panic() {
        // Empty, tiny, header-only, and randomly-truncated frames all return None.
        assert!(parse_nflog_message(&[]).is_none());
        assert!(parse_nflog_message(&[0u8; 4]).is_none());
        assert!(parse_nflog_message(&[0u8; 19]).is_none());
        let full = nflog_msg("elastos-egress-drop:cvtap ", &ipv4([1, 2, 3, 4], 6, 443));
        for cut in 0..full.len() {
            // Must never panic, whatever the truncation point.
            let _ = parse_nflog_message(&full[..cut]);
        }
        // A PREFIX present but a too-short / non-IPv4 payload → None, not a crash.
        assert!(parse_nflog_message(&nflog_msg("elastos-egress-drop:cvtap ", &[0u8; 3])).is_none());
        let mut not_v4 = ipv4([1, 2, 3, 4], 6, 443);
        not_v4[0] = 0x65; // version 6
        assert!(parse_nflog_message(&nflog_msg("elastos-egress-drop:cvtap ", &not_v4)).is_none());
    }

    #[test]
    fn oversized_or_empty_tap_in_prefix_is_rejected() {
        // An interface name can't exceed 15 chars; a forged over-long tap is dropped.
        let long = "elastos-egress-drop:cvthisistoolong12345 ";
        assert!(parse_nflog_message(&nflog_msg(long, &ipv4([1, 2, 3, 4], 6, 1))).is_none());
        // Empty tap after the tag.
        assert!(parse_nflog_message(&nflog_msg(
            "elastos-egress-drop: ",
            &ipv4([1, 2, 3, 4], 6, 1)
        ))
        .is_none());
    }
}

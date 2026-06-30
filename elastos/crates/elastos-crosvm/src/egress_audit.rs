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

/// NFLOG message type for a logged packet: `(NFNL_SUBSYS_ULOG << 8) |
/// NFULNL_MSG_PACKET`, i.e. subsystem 5, message 0.
const NFULNL_MSG_PACKET_TYPE: u16 = 5 << 8;
/// `struct nlmsghdr` size.
const NLMSG_HDR_LEN: usize = 16;
/// `struct nfgenmsg` size (family, version, res_id).
const NFGENMSG_LEN: usize = 4;
/// NFULA_PREFIX attribute — the `log prefix` string we keyed with the TAP.
const NFULA_PREFIX: u16 = 2;
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
    const NFULNL_MSG_CONFIG_TYPE: u16 = (5 << 8) | 1;
    const NLM_F_REQUEST: u16 = 0x01;
    const NFULA_CFG_CMD: u16 = 1;
    const NFULA_CFG_MODE: u16 = 2;
    const NFULNL_CFG_CMD_BIND: u8 = 1;
    const NFULNL_COPY_PACKET: u8 = 2;

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
            // Bind the group, then request the full packet copy mode.
            reader.send_config(group, &cfg_cmd_attr(NFULNL_CFG_CMD_BIND))?;
            reader.send_config(group, &cfg_mode_attr(NFULNL_COPY_PACKET, 0xffff))?;
            Ok(reader)
        }

        /// Send one NFULNL_MSG_CONFIG message carrying a single attribute.
        fn send_config(&self, group: u16, attr: &[u8]) -> Result<()> {
            let total = 16 + 4 + attr.len();
            let mut msg = Vec::with_capacity(total);
            // nlmsghdr
            msg.extend_from_slice(&(total as u32).to_ne_bytes());
            msg.extend_from_slice(&NFULNL_MSG_CONFIG_TYPE.to_ne_bytes());
            msg.extend_from_slice(&NLM_F_REQUEST.to_ne_bytes());
            msg.extend_from_slice(&0u32.to_ne_bytes()); // seq
            msg.extend_from_slice(&0u32.to_ne_bytes()); // pid
                                                        // nfgenmsg: family=AF_UNSPEC, version=0, res_id=htons(group)
            msg.push(0); // nfgen_family
            msg.push(0); // version
            msg.extend_from_slice(&group.to_be_bytes()); // res_id (network order)
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

    /// `NFULA_CFG_CMD` attribute carrying a 1-byte command (4-byte header + pad).
    fn cfg_cmd_attr(command: u8) -> Vec<u8> {
        let mut a = Vec::new();
        a.extend_from_slice(&5u16.to_ne_bytes()); // nla_len = 4 + 1
        a.extend_from_slice(&NFULA_CFG_CMD.to_ne_bytes());
        a.push(command);
        while a.len() % 4 != 0 {
            a.push(0);
        }
        a
    }

    /// `NFULA_CFG_MODE` attribute: `__be32 copy_range; u8 copy_mode; u8 _pad`.
    fn cfg_mode_attr(copy_mode: u8, copy_range: u32) -> Vec<u8> {
        let mut a = Vec::new();
        a.extend_from_slice(&10u16.to_ne_bytes()); // nla_len = 4 + 6
        a.extend_from_slice(&NFULA_CFG_MODE.to_ne_bytes());
        a.extend_from_slice(&copy_range.to_be_bytes()); // network order
        a.push(copy_mode);
        a.push(0); // pad inside the struct
        while a.len() % 4 != 0 {
            a.push(0);
        }
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

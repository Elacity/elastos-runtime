use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

const MAX_CLIENTS: usize = 256;
const SHORT_WINDOW: Duration = Duration::from_secs(60);
const CHALLENGE_WINDOW: Duration = Duration::from_secs(300);
const MAX_SHORT_BEGINS: usize = 4;
// Count starts for the complete challenge lifetime. Even a client that changes
// its registration cookie can own at most this many pending starts per kind.
const MAX_PENDING_STARTS: usize = 8;

#[derive(Clone, Copy)]
enum CeremonyKind {
    Registration,
    Authentication,
}

impl CeremonyKind {
    fn from_path(path: &str) -> Option<Self> {
        match path {
            "/api/auth/passkey/register/begin" => Some(Self::Registration),
            "/api/auth/passkey/authenticate/begin" => Some(Self::Authentication),
            _ => None,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Registration => 0,
            Self::Authentication => 1,
        }
    }
}

#[derive(Default)]
struct BeginLimiter {
    clients: BTreeMap<(PathBuf, Option<IpAddr>), [VecDeque<Instant>; 2]>,
}

impl BeginLimiter {
    fn admit(
        &mut self,
        root: &Path,
        peer: Option<IpAddr>,
        kind: CeremonyKind,
        now: Instant,
    ) -> bool {
        self.clients.retain(|_, starts| {
            for window in starts.iter_mut() {
                while window
                    .front()
                    .is_some_and(|start| now.saturating_duration_since(*start) >= CHALLENGE_WINDOW)
                {
                    window.pop_front();
                }
            }
            starts.iter().any(|window| !window.is_empty())
        });
        let key = (root.to_path_buf(), peer.map(|ip| ip.to_canonical()));
        if !self.clients.contains_key(&key) && self.clients.len() >= MAX_CLIENTS {
            return false;
        }
        let window = &mut self.clients.entry(key).or_default()[kind.index()];
        if window.len() >= MAX_PENDING_STARTS
            || window
                .iter()
                .filter(|start| now.saturating_duration_since(**start) < SHORT_WINDOW)
                .count()
                >= MAX_SHORT_BEGINS
        {
            return false;
        }
        window.push_back(now);
        true
    }
}

/// Gateway calls this after Host/origin validation and before body extraction.
/// The socket peer owns the quota; forwarded headers and cookies cannot select it.
pub(in crate::api) fn admit_passkey_begin(
    data_dir: &Path,
    peer: Option<SocketAddr>,
    path: &str,
) -> Result<(), Response> {
    let Some(kind) = CeremonyKind::from_path(path) else {
        return Ok(());
    };
    static LIMITER: OnceLock<Mutex<BeginLimiter>> = OnceLock::new();
    let admitted = LIMITER
        .get_or_init(|| Mutex::new(BeginLimiter::default()))
        .lock()
        .map(|mut limiter| {
            limiter.admit(data_dir, peer.map(|peer| peer.ip()), kind, Instant::now())
        })
        .unwrap_or(false);
    if admitted {
        Ok(())
    } else {
        Err((
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "60")],
            "Passkey sign-in is busy. Try again shortly.",
        )
            .into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_quota_is_separate_for_each_peer_kind_and_home() {
        let mut limiter = BeginLimiter::default();
        let root = Path::new("home-a");
        let attacker = Some("192.0.2.1".parse().unwrap());
        let guest = Some("192.0.2.2".parse().unwrap());
        let now = Instant::now();
        for _ in 0..MAX_SHORT_BEGINS {
            assert!(limiter.admit(root, attacker, CeremonyKind::Registration, now));
            assert!(limiter.admit(root, attacker, CeremonyKind::Authentication, now));
        }
        assert!(!limiter.admit(root, attacker, CeremonyKind::Registration, now));
        assert!(!limiter.admit(root, attacker, CeremonyKind::Authentication, now));
        assert!(limiter.admit(root, guest, CeremonyKind::Registration, now));
        assert!(limiter.admit(root, guest, CeremonyKind::Authentication, now));
        assert!(limiter.admit(
            Path::new("home-b"),
            attacker,
            CeremonyKind::Registration,
            now
        ));
        // IPv4-mapped IPv6 shares the IPv4 quota.
        assert!(!limiter.admit(
            root,
            Some("::ffff:192.0.2.1".parse().unwrap()),
            CeremonyKind::Registration,
            now
        ));
    }

    #[test]
    fn pending_start_budget_recovers_only_after_challenge_expiry() {
        let mut limiter = BeginLimiter::default();
        let root = Path::new("home");
        let now = Instant::now();
        for step in [Duration::ZERO, SHORT_WINDOW] {
            for _ in 0..MAX_SHORT_BEGINS {
                assert!(limiter.admit(root, None, CeremonyKind::Authentication, now + step));
            }
        }
        assert!(!limiter.admit(
            root,
            None,
            CeremonyKind::Authentication,
            now + SHORT_WINDOW * 2
        ));
        assert!(limiter.admit(
            root,
            None,
            CeremonyKind::Authentication,
            now + CHALLENGE_WINDOW
        ));
        assert_eq!(limiter.clients.len(), 1);
    }

    #[test]
    fn caller_table_is_bounded_and_keeps_existing_guest_quota() {
        let mut limiter = BeginLimiter::default();
        let root = Path::new("home");
        let now = Instant::now();
        for client in 0..MAX_CLIENTS {
            let peer = Some(IpAddr::V4(std::net::Ipv4Addr::from(
                0xc000_0200u32 + client as u32,
            )));
            assert!(limiter.admit(root, peer, CeremonyKind::Registration, now));
        }
        assert!(!limiter.admit(
            root,
            Some("198.51.100.1".parse().unwrap()),
            CeremonyKind::Registration,
            now
        ));
        assert_eq!(limiter.clients.len(), MAX_CLIENTS);
        assert!(limiter.admit(
            root,
            Some("192.0.2.0".parse().unwrap()),
            CeremonyKind::Authentication,
            now
        ));
        assert!(limiter.admit(
            root,
            Some("198.51.100.1".parse().unwrap()),
            CeremonyKind::Registration,
            now + CHALLENGE_WINDOW
        ));
        assert_eq!(limiter.clients.len(), 1);
    }

    #[test]
    fn completions_have_no_begin_quota() {
        for path in [
            "/api/auth/passkey/register/complete",
            "/api/auth/passkey/authenticate/complete",
        ] {
            assert!(admit_passkey_begin(Path::new("home"), None, path).is_ok());
        }
    }
}

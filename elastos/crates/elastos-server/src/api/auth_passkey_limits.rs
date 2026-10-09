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
    ) -> Result<(), Duration> {
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
            // An entry leaves the table when its last start expires in either pool.
            let retry = self
                .clients
                .values()
                .filter_map(|starts| starts.iter().flat_map(|window| window.back()).max())
                .map(|last| CHALLENGE_WINDOW.saturating_sub(now.saturating_duration_since(*last)))
                .min()
                .expect("a full caller table has retained starts");
            return Err(retry);
        }
        let window = &mut self.clients.entry(key).or_default()[kind.index()];
        let mut retry = Duration::ZERO;
        if window.len() >= MAX_PENDING_STARTS {
            let start = window[window.len() - MAX_PENDING_STARTS];
            retry = CHALLENGE_WINDOW.saturating_sub(now.saturating_duration_since(start));
        }
        if let Some(start) = window.iter().rev().nth(MAX_SHORT_BEGINS - 1) {
            retry = retry.max(SHORT_WINDOW.saturating_sub(now.saturating_duration_since(*start)));
        }
        if !retry.is_zero() {
            return Err(retry);
        }
        window.push_back(now);
        Ok(())
    }
}

fn begin_limit_response(kind: CeremonyKind, retry: Duration) -> Response {
    // A fractional second still needs one complete second before retrying.
    let seconds = retry.as_secs() + u64::from(retry.subsec_nanos() != 0);
    let action = match kind {
        CeremonyKind::Registration => "sign-up",
        CeremonyKind::Authentication => "sign-in",
    };
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("retry-after", seconds.to_string())],
        format!("Too many passkey {action} attempts. Try again in {seconds} seconds."),
    )
        .into_response()
}

/// Gateway calls this after Host/origin validation and before body extraction.
/// The socket peer owns the quota; forwarded headers and cookies cannot select it.
pub(in crate::api) fn admit_passkey_begin(
    data_dir: &Path,
    peer: Option<SocketAddr>,
    path: &str,
) -> Result<(), Box<Response>> {
    let Some(kind) = CeremonyKind::from_path(path) else {
        return Ok(());
    };
    static LIMITER: OnceLock<Mutex<BeginLimiter>> = OnceLock::new();
    let Ok(mut limiter) = LIMITER
        .get_or_init(|| Mutex::new(BeginLimiter::default()))
        .lock()
    else {
        return Err(Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Passkey setup is unavailable. Try again later.",
            )
                .into_response(),
        ));
    };
    match limiter.admit(data_dir, peer.map(|peer| peer.ip()), kind, Instant::now()) {
        Ok(()) => Ok(()),
        Err(retry) => Err(Box::new(begin_limit_response(kind, retry))),
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
            assert!(limiter
                .admit(root, attacker, CeremonyKind::Registration, now)
                .is_ok());
            assert!(limiter
                .admit(root, attacker, CeremonyKind::Authentication, now)
                .is_ok());
        }
        assert_eq!(
            limiter.admit(root, attacker, CeremonyKind::Registration, now),
            Err(SHORT_WINDOW)
        );
        assert!(limiter
            .admit(root, attacker, CeremonyKind::Authentication, now)
            .is_err());
        assert!(limiter
            .admit(root, guest, CeremonyKind::Registration, now)
            .is_ok());
        assert!(limiter
            .admit(root, guest, CeremonyKind::Authentication, now)
            .is_ok());
        assert!(limiter
            .admit(
                Path::new("home-b"),
                attacker,
                CeremonyKind::Registration,
                now
            )
            .is_ok());
        // IPv4-mapped IPv6 shares the IPv4 quota.
        assert!(limiter
            .admit(
                root,
                Some("::ffff:192.0.2.1".parse().unwrap()),
                CeremonyKind::Registration,
                now
            )
            .is_err());
    }

    #[test]
    fn pending_start_budget_recovers_only_after_challenge_expiry() {
        let mut limiter = BeginLimiter::default();
        let root = Path::new("home");
        let now = Instant::now();
        for step in [Duration::ZERO, SHORT_WINDOW] {
            for _ in 0..MAX_SHORT_BEGINS {
                assert!(limiter
                    .admit(root, None, CeremonyKind::Authentication, now + step)
                    .is_ok());
            }
        }
        assert_eq!(
            limiter.admit(
                root,
                None,
                CeremonyKind::Authentication,
                now + SHORT_WINDOW * 2
            ),
            Err(Duration::from_secs(180))
        );
        assert_eq!(
            limiter.admit(
                root,
                None,
                CeremonyKind::Authentication,
                now + CHALLENGE_WINDOW - Duration::from_millis(500)
            ),
            Err(Duration::from_millis(500))
        );
        assert!(limiter
            .admit(
                root,
                None,
                CeremonyKind::Authentication,
                now + CHALLENGE_WINDOW
            )
            .is_ok());
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
            assert!(limiter
                .admit(root, peer, CeremonyKind::Registration, now)
                .is_ok());
        }
        assert_eq!(
            limiter.admit(
                root,
                Some("198.51.100.1".parse().unwrap()),
                CeremonyKind::Registration,
                now
            ),
            Err(CHALLENGE_WINDOW)
        );
        assert_eq!(limiter.clients.len(), MAX_CLIENTS);
        assert!(limiter
            .admit(
                root,
                Some("192.0.2.0".parse().unwrap()),
                CeremonyKind::Authentication,
                now
            )
            .is_ok());
        // Renew each entry's other pool: the older registration start alone
        // cannot predict when a caller-table slot becomes available.
        for client in 0..MAX_CLIENTS {
            let peer = Some(IpAddr::V4(std::net::Ipv4Addr::from(
                0xc000_0200u32 + client as u32,
            )));
            assert!(limiter
                .admit(root, peer, CeremonyKind::Authentication, now + SHORT_WINDOW,)
                .is_ok());
        }
        assert_eq!(
            limiter.admit(
                root,
                Some("198.51.100.1".parse().unwrap()),
                CeremonyKind::Registration,
                now + CHALLENGE_WINDOW,
            ),
            Err(SHORT_WINDOW)
        );
        assert!(limiter
            .admit(
                root,
                Some("198.51.100.1".parse().unwrap()),
                CeremonyKind::Registration,
                now + CHALLENGE_WINDOW + SHORT_WINDOW
            )
            .is_ok());
        assert_eq!(limiter.clients.len(), 1);
    }

    #[test]
    fn retry_waits_until_both_rate_windows_allow_a_start() {
        let mut limiter = BeginLimiter::default();
        let root = Path::new("home");
        let now = Instant::now();
        for offset in [0, 10, 20, 30, 60, 70, 80, 90] {
            assert!(limiter
                .admit(
                    root,
                    None,
                    CeremonyKind::Registration,
                    now + Duration::from_secs(offset),
                )
                .is_ok());
        }
        // The one-minute window opens at 120 s, but the eighth-start pool
        // remains full until the oldest challenge expires at 300 s.
        assert_eq!(
            limiter.admit(
                root,
                None,
                CeremonyKind::Registration,
                now + Duration::from_secs(100),
            ),
            Err(Duration::from_secs(200))
        );
        assert!(limiter
            .admit(
                root,
                None,
                CeremonyKind::Registration,
                now + CHALLENGE_WINDOW,
            )
            .is_ok());
        assert_eq!(
            limiter.admit(
                root,
                None,
                CeremonyKind::Registration,
                now + CHALLENGE_WINDOW,
            ),
            Err(Duration::from_secs(10))
        );
    }

    #[tokio::test]
    async fn retry_response_names_the_action_and_rounds_up_partial_seconds() {
        use axum::body::to_bytes;

        for (kind, action) in [
            (CeremonyKind::Registration, "sign-up"),
            (CeremonyKind::Authentication, "sign-in"),
        ] {
            let response = begin_limit_response(kind, Duration::from_millis(180_500));
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()["retry-after"], "181");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                std::str::from_utf8(&body).unwrap(),
                format!("Too many passkey {action} attempts. Try again in 181 seconds.")
            );
        }
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

//! Slowing down password guessing: failed sign-ins are counted by client address and by user
//! name. The first [`FREE_TRIES`] failures in a row cost nothing (typos); after that every
//! further try has to wait, doubling from a second up to [`MAX_WAIT`]. A successful sign-in
//! clears its counters.

use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const FREE_TRIES: u32 = 5;
pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// Counters kept at most (the oldest are dropped first).
const KEPT: usize = 10_000;

#[derive(Default)]
pub struct Throttle {
    /// Failures in a row, and when the next try is allowed.
    fails: HashMap<String, (u32, Instant)>,
}

impl Throttle {
    /// How long these keys must wait before trying again (`None`: go ahead).
    pub fn wait(&self, keys: &[String]) -> Option<Duration> {
        let now = Instant::now();
        keys.iter().filter_map(|k| self.fails.get(k)).filter_map(|(_, at)| at.checked_duration_since(now)).max()
    }

    pub fn failed(&mut self, keys: &[String]) {
        let now = Instant::now();
        if self.fails.len() >= KEPT {
            self.fails.retain(|_, (_, at)| *at > now);
            if self.fails.len() >= KEPT {
                self.fails.clear();
            }
        }
        for k in keys {
            let e = self.fails.entry(k.clone()).or_insert((0, now));
            e.0 = e.0.saturating_add(1);
            if e.0 > FREE_TRIES {
                let wait = Duration::from_secs(1u64 << (e.0 - FREE_TRIES - 1).min(20)).min(MAX_WAIT);
                e.1 = now + wait;
            }
        }
    }

    pub fn succeeded(&mut self, keys: &[String]) {
        for k in keys {
            self.fails.remove(k);
        }
    }
}

/// The address a request came from: the peer, or — when the peer is this machine or a private
/// network (a reverse proxy such as Caddy) — the last address in its `X-Forwarded-For`.
pub fn client(peer: Option<std::net::SocketAddr>, forwarded: Option<&str>) -> String {
    let Some(peer) = peer else { return "unknown".into() };
    let ip = peer.ip();
    let proxied = match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.segments().first().is_some_and(|s| s & 0xfe00 == 0xfc00),
    };
    if proxied && let Some(last) = forwarded.and_then(|f| f.rsplit(',').next()).map(str::trim).filter(|s| !s.is_empty() && s.len() <= 64) {
        return last.to_string();
    }
    ip.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typos_are_free_guessing_waits() {
        let mut t = Throttle::default();
        let k = vec!["ip:1".to_string(), "user:ann".to_string()];
        for _ in 0..FREE_TRIES {
            t.failed(&k);
            assert_eq!(t.wait(&k), None);
        }
        t.failed(&k);
        assert!(t.wait(&k).is_some_and(|w| w <= Duration::from_secs(1)));
        assert!(t.wait(&["user:ann".to_string()]).is_some(), "by name too, from any address");
        assert_eq!(t.wait(&["ip:2".to_string()]), None);
        for _ in 0..40 {
            t.failed(&k);
        }
        assert!(t.wait(&k).is_some_and(|w| w <= MAX_WAIT));
        t.succeeded(&k);
        assert_eq!(t.wait(&k), None);
        let peer = |s: &str| Some(s.parse().unwrap());
        assert_eq!(client(peer("203.0.113.9:5000"), Some("1.2.3.4")), "203.0.113.9", "a public peer can't name another address");
        assert_eq!(client(peer("172.18.0.3:5000"), Some("6.6.6.6, 198.51.100.7")), "198.51.100.7");
        assert_eq!(client(peer("127.0.0.1:5000"), None), "127.0.0.1");
    }
}

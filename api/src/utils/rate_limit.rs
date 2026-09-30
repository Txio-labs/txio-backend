//! Per-API-key rate limiting for the public API.
//!
//! The app-wide limiter (main.rs) is keyed on the peer IP, which is the wrong
//! identity for API traffic: many keys can share one NAT, and one key can be
//! spread over many IPs. Here the bucket belongs to (key, endpoint class), so a
//! noisy `execute` caller cannot use up a key's `history` budget and vice versa.
//! State is in memory, so with more than one API instance each instance
//! enforces its own copy of the limit; share it through Redis if that matters.

use mongodb::bson::oid::ObjectId;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    Read,
    Simulate,
    Execute,
}

impl EndpointClass {
    /// Class for a request path relative to `/api/public/v1`.
    pub fn for_path(path: &str) -> Self {
        if path.ends_with("/transactions/execute") {
            EndpointClass::Execute
        } else if path.ends_with("/transactions/simulate") {
            EndpointClass::Simulate
        } else {
            EndpointClass::Read
        }
    }
}

/// Requests per minute for each class.
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
    pub read: u32,
    pub simulate: u32,
    pub execute: u32,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self { read: 60, simulate: 30, execute: 10 }
    }
}

impl RateLimits {
    /// Reads `TXIO_API_RATE_READ`, `TXIO_API_RATE_SIMULATE`, `TXIO_API_RATE_EXECUTE`
    /// (requests per minute), falling back to the defaults.
    pub fn from_env() -> Self {
        let get = |name: &str, default: u32| {
            std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(default)
        };
        let d = Self::default();
        Self {
            read: get("TXIO_API_RATE_READ", d.read),
            simulate: get("TXIO_API_RATE_SIMULATE", d.simulate),
            execute: get("TXIO_API_RATE_EXECUTE", d.execute),
        }
    }

    pub fn for_class(&self, class: EndpointClass) -> u32 {
        match class {
            EndpointClass::Read => self.read,
            EndpointClass::Simulate => self.simulate,
            EndpointClass::Execute => self.execute,
        }
    }
}

/// What a caller is told about its budget, for `X-RateLimit-*` headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub limit: u32,
    pub remaining: u32,
    /// Seconds until the bucket is full again.
    pub reset_secs: u64,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct KeyRateLimiter {
    limits: RateLimits,
    buckets: Mutex<HashMap<(ObjectId, EndpointClass), Bucket>>,
}

impl KeyRateLimiter {
    pub fn new(limits: RateLimits) -> Self {
        Self { limits, buckets: Mutex::new(HashMap::new()) }
    }

    pub fn limit_for(&self, class: EndpointClass) -> u32 {
        self.limits.for_class(class)
    }

    /// Takes one token. `Ok` carries the remaining quota; `Err` the seconds to
    /// wait before a token is available.
    pub fn check(&self, key: ObjectId, class: EndpointClass, now: Instant) -> Result<Quota, u64> {
        let limit = self.limits.for_class(class);
        let per_second = limit as f64 / 60.0;
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        // Idle buckets are full buckets; dropping them changes nothing and
        // keeps the map from growing with every key ever seen.
        if buckets.len() > 10_000 {
            buckets.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(600));
        }

        let bucket = buckets.entry((key, class)).or_insert(Bucket { tokens: limit as f64, last: now });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * per_second).min(limit as f64);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            let missing = limit as f64 - bucket.tokens;
            Ok(Quota {
                limit,
                remaining: bucket.tokens.floor() as u32,
                reset_secs: (missing / per_second).ceil() as u64,
            })
        } else {
            Err(((1.0 - bucket.tokens) / per_second).ceil().max(1.0) as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> KeyRateLimiter {
        KeyRateLimiter::new(RateLimits { read: 60, simulate: 30, execute: 2 })
    }

    #[test]
    fn allows_the_limit_then_rejects_with_retry_after() {
        let l = limiter();
        let key = ObjectId::new();
        let t0 = Instant::now();
        let first = l.check(key, EndpointClass::Execute, t0).unwrap();
        assert_eq!((first.limit, first.remaining), (2, 1));
        assert_eq!(l.check(key, EndpointClass::Execute, t0).unwrap().remaining, 0);
        // 2/min refills one token every 30 s.
        assert_eq!(l.check(key, EndpointClass::Execute, t0).unwrap_err(), 30);
    }

    #[test]
    fn refills_over_time_but_never_past_the_limit() {
        let l = limiter();
        let key = ObjectId::new();
        let t0 = Instant::now();
        l.check(key, EndpointClass::Execute, t0).unwrap();
        l.check(key, EndpointClass::Execute, t0).unwrap();
        assert!(l.check(key, EndpointClass::Execute, t0 + Duration::from_secs(29)).is_err());
        assert!(l.check(key, EndpointClass::Execute, t0 + Duration::from_secs(31)).is_ok());
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(l.check(key, EndpointClass::Execute, later).unwrap().remaining, 1);
    }

    #[test]
    fn keys_and_classes_have_separate_budgets() {
        let l = limiter();
        let (a, b) = (ObjectId::new(), ObjectId::new());
        let t0 = Instant::now();
        l.check(a, EndpointClass::Execute, t0).unwrap();
        l.check(a, EndpointClass::Execute, t0).unwrap();
        assert!(l.check(a, EndpointClass::Execute, t0).is_err());
        assert!(l.check(b, EndpointClass::Execute, t0).is_ok());
        assert!(l.check(a, EndpointClass::Read, t0).is_ok());
    }

    #[test]
    fn classifies_paths() {
        assert_eq!(EndpointClass::for_path("/api/public/v1/transactions/execute"), EndpointClass::Execute);
        assert_eq!(EndpointClass::for_path("/transactions/simulate"), EndpointClass::Simulate);
        assert_eq!(EndpointClass::for_path("/history"), EndpointClass::Read);
    }
}

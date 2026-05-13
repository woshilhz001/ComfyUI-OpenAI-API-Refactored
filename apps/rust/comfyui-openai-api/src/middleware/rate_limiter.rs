// src/middleware/rate_limiter.rs
use std::sync::Mutex;
use std::time::Instant;

struct Inner {
    tokens: f64,
    last: Instant,
}

pub struct RateLimiter {
    capacity: u64,
    rate: f64,
    inner: Mutex<Inner>,
}

impl RateLimiter {
    pub fn new(max_tokens: u64, refill_rate: f64) -> Self {
        Self {
            capacity: max_tokens,
            rate: refill_rate,
            inner: Mutex::new(Inner {
                tokens: max_tokens as f64,
                last: Instant::now(),
            }),
        }
    }

    pub fn try_acquire(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        let elapsed = (now - inner.last).as_secs_f64();
        inner.tokens = (inner.tokens + elapsed * self.rate).min(self.capacity as f64);
        inner.last = now;
        if inner.tokens >= 1.0 {
            inner.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
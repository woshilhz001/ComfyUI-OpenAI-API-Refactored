use std::sync::Mutex;
use std::time::Instant;

pub struct RateLimiter {
    capacity: u64,
    tokens: f64,
    rate: f64,
    last: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(max_tokens: u64, refill_rate: f64) -> Self {
        Self { capacity: max_tokens, tokens: max_tokens as f64, rate: refill_rate, last: Mutex::new(Instant::now()) }
    }

    pub fn try_acquire(&self) -> bool {
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        let elapsed = (now - *last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.capacity as f64);
        *last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else { false }
    }
}
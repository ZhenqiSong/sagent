use std::time::{Duration, Instant};

/// 令牌桶速率限制器——基于 token bucket 算法。
/// 支持 `try_acquire()` 非阻塞获取和 `acquire_blocking()` 阻塞等待。
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// 桶容量（最大令牌数）
    capacity: u32,
    /// 当前可用令牌数（不超过 capacity）
    tokens: f64,
    /// 每秒补充的令牌数
    refill_per_sec: f64,
    /// 上次补充令牌的时间点
    last_refill: Instant,
}

impl TokenBucket {
    /// 创建一个新的令牌桶。
    ///
    /// # 参数
    /// - `capacity`: 桶容量，即最多可积累的令牌数。
    /// - `refill_per_sec`: 每秒补充的令牌数。
    pub fn new(capacity: u32, refill_per_sec: f64) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            refill_per_sec,
            last_refill: Instant::now(),
        }
    }

    /// 尝试获取 `n` 个令牌。
    ///
    /// 返回 `true` 表示获取成功（已扣除令牌），`false` 表示令牌不足。
    /// 调用前会自动补充自上次调用以来新生成的令牌。
    pub fn try_acquire(&mut self, n: u32) -> bool {
        self.refill();
        let needed = n as f64;
        if self.tokens >= needed {
            self.tokens -= needed;
            true
        } else {
            false
        }
    }

    /// 阻塞获取 `n` 个令牌，直到令牌足够。
    ///
    /// 通过计算还需等待的时间并调用 `std::thread::sleep` 实现阻塞。
    /// 注意：此方法会阻塞当前线程，仅适用于非异步上下文。异步场景请使用
    /// `try_acquire` + `tokio::time::sleep` 自行轮询。
    pub fn acquire_blocking(&mut self, n: u32) {
        self.refill();
        let needed = n as f64;
        if self.tokens >= needed {
            self.tokens -= needed;
            return;
        }

        // 计算还需要等待的时长
        let deficit = needed - self.tokens;
        let wait_secs = deficit / self.refill_per_sec;
        std::thread::sleep(Duration::from_secs_f64(wait_secs));

        // 等待后再次补充并扣除
        self.refill();
        self.tokens -= needed;
    }

    /// 获取当前可用令牌数（会先补充）。
    pub fn available_tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// 重置令牌桶到满容量状态。
    pub fn reset(&mut self) {
        self.tokens = self.capacity as f64;
        self.last_refill = Instant::now();
    }

    /// 内部方法：根据时间差补充令牌，不超过 capacity。
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let new_tokens = elapsed * self.refill_per_sec;
        self.tokens = (self.tokens + new_tokens).min(self.capacity as f64);
        self.last_refill = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bucket_is_full() {
        let mut bucket = TokenBucket::new(10, 5.0);
        assert!(bucket.available_tokens() >= 9.9); // 浮点容忍
    }

    #[test]
    fn test_try_acquire_success() {
        let mut bucket = TokenBucket::new(10, 5.0);
        assert!(bucket.try_acquire(3));
        assert!(bucket.try_acquire(5));
        // 剩余约 2 个令牌
        assert!(bucket.available_tokens() < 3.0);
    }

    #[test]
    fn test_try_acquire_insufficient() {
        let mut bucket = TokenBucket::new(3, 1.0);
        assert!(bucket.try_acquire(2)); // 成功
        assert!(!bucket.try_acquire(2)); // 只剩约 1 个，失败
        assert!(!bucket.try_acquire(5)); // 超出容量，失败
    }

    #[test]
    fn test_acquire_blocking_eventually_succeeds() {
        let mut bucket = TokenBucket::new(5, 100.0); // 每秒 100 个令牌，很快补充
        // 先耗尽
        bucket.try_acquire(5);
        // 阻塞获取 1 个，应该很快等到
        bucket.acquire_blocking(1);
        // 不应 panic，能正常返回
    }

    #[test]
    fn test_reset_restores_full_capacity() {
        let mut bucket = TokenBucket::new(10, 5.0);
        bucket.try_acquire(8);
        assert!(bucket.available_tokens() < 3.0);
        bucket.reset();
        assert!(bucket.available_tokens() >= 9.9);
    }

    #[test]
    fn test_tokens_never_exceed_capacity() {
        let mut bucket = TokenBucket::new(5, 1000.0);
        // 即使补充速率很高，也不应超过容量
        bucket.reset();
        // 等待一小段时间让令牌补充
        std::thread::sleep(Duration::from_millis(50));
        let available = bucket.available_tokens();
        assert!(available <= 5.0 + f64::EPSILON);
    }

    #[test]
    fn test_refill_accumulates_over_time() {
        let mut bucket = TokenBucket::new(100, 10.0); // 每秒 10 个
        bucket.try_acquire(100); // 清空
        assert!(bucket.available_tokens() < 0.01);

        // 等待约 0.2 秒，应补充约 2 个令牌
        std::thread::sleep(Duration::from_millis(200));
        let available = bucket.available_tokens();
        assert!(available >= 1.5 && available <= 3.0, "available={available}");
    }

    #[test]
    fn test_try_acquire_zero() {
        let mut bucket = TokenBucket::new(5, 1.0);
        // 获取 0 个令牌应该总是成功
        assert!(bucket.try_acquire(0));
        // 令牌数不变
        assert!(bucket.available_tokens() >= 4.9);
    }
}

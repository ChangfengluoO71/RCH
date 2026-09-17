//! 按 key 合并并发调用（singleflight）。
//!
//! # 为什么需要
//!
//! `Cloud115WebClient::downurl` 原来的做法是"缓存未命中就抢一把**全客户端**的
//! `Mutex<()>`"，而那把锁覆盖了限速等待 + 一次 HTTP POST（最多约 1.2 s）。
//! 后果是：后台给漫画 A 取链时，阅读打开漫画 B 会在**同一个锁**上等满这段时间
//! —— 不同文件之间被无谓地串行化。P0-C 用按 key 的合并替换它。
//!
//! # 语义
//!
//! - **同 key 合并**：并发的同 key 调用只有一个 leader 真正执行，其余 follower
//!   等待并拿到**完全相同**的结果（成功或失败）。
//! - **不同 key 互不阻塞**：内部 map 锁只在"登记/摘除 slot"时短暂持有，
//!   绝不跨越被合并的操作本身。
//! - **失败一致传播**：leader 的错误会被 follower 原样拿到，不会各自重试形成
//!   stampede。
//! - **取消不做**：已被合并的调用要么等 leader 出结果，要么自己超时；本模块
//!   不提供"取消别人的 load"。这是刻意的 —— 见审阅冻结决策 4（不打断在途请求）。
//!
//! # 不做缓存
//!
//! 本模块只管"合并"，不管"记住"。缓存生命周期（TTL、403 失效、鉴权失败后刷新）
//! 由调用方按各自 provider 的契约决定 —— 115 与夸克的契约不同，不能共用一套。

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

struct Slot<V> {
    value: Mutex<Option<V>>,
    ready: Condvar,
}

/// 一次合并调用的结果。
pub struct SingleFlightOutcome<V> {
    /// 本次调用是否是真正执行 load 的那个（false = 搭了别人的车）。
    pub leader: bool,
    pub value: V,
    /// 搭车时等待 leader 的微秒数；leader 恒为 0。
    pub waited_us: u64,
}

pub struct SingleFlight<K, V> {
    slots: Mutex<HashMap<K, Arc<Slot<V>>>>,
}

impl<K, V> Default for SingleFlight<K, V> {
    fn default() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
        }
    }
}

impl<V> Slot<V> {
    fn new() -> Self {
        Slot {
            value: Mutex::new(None),
            ready: Condvar::new(),
        }
    }
}

impl<K, V> SingleFlight<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前正在合并的 key 数量（仅用于观测与测试）。
    pub fn inflight_keys(&self) -> usize {
        self.slots.lock().unwrap().len()
    }

    /// 以 `key` 为粒度合并并发调用。
    ///
    /// leader = **登记 slot 成功**（即 map 中原本没有该 key）的那个调用；只有它
    /// 执行 `load`，且执行期间不持有任何内部锁。其余调用作为 follower 等待并拿到
    /// 同一个值（成功或失败都一样）。
    pub fn run<F>(&self, key: K, load: F) -> SingleFlightOutcome<V>
    where
        F: FnOnce() -> V,
    {
        let (slot, leader) = {
            let mut slots = self.slots.lock().unwrap();
            match slots.get(&key) {
                Some(existing) => (Arc::clone(existing), false),
                None => {
                    let slot = Arc::new(Slot::new());
                    slots.insert(key.clone(), Arc::clone(&slot));
                    (slot, true)
                }
            }
        };

        if leader {
            // 不持有 map 锁、也不持有 slot 锁地执行真正的 load。
            let value = load();
            {
                let mut guard = slot.value.lock().unwrap();
                *guard = Some(value.clone());
            }
            slot.ready.notify_all();
            {
                let mut slots = self.slots.lock().unwrap();
                slots.remove(&key);
            }
            return SingleFlightOutcome {
                leader: true,
                value,
                waited_us: 0,
            };
        }

        // follower：等 leader 落值。分片等待以免 leader panic 时永久卡死。
        let started = Instant::now();
        let mut guard = slot.value.lock().unwrap();
        while guard.is_none() {
            let (next, _) = slot
                .ready
                .wait_timeout(guard, std::time::Duration::from_millis(50))
                .unwrap();
            guard = next;
        }
        let value = guard.as_ref().cloned().expect("value was just set");
        SingleFlightOutcome {
            leader: false,
            value,
            waited_us: started.elapsed().as_micros() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn same_key_runs_the_load_once_and_shares_the_value() {
        let flight: Arc<SingleFlight<String, u32>> = Arc::new(SingleFlight::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let flight = Arc::clone(&flight);
            let calls = Arc::clone(&calls);
            handles.push(std::thread::spawn(move || {
                let outcome = flight.run("same".to_string(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(60));
                    7_u32
                });
                outcome.value
            }));
        }
        let values: Vec<u32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(values.iter().all(|v| *v == 7));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "concurrent callers for one key must collapse into a single load"
        );
    }

    #[test]
    fn different_keys_are_not_serialised() {
        let flight: Arc<SingleFlight<String, u32>> = Arc::new(SingleFlight::new());
        let started = Instant::now();
        let mut handles = Vec::new();
        for index in 0..4_u32 {
            let flight = Arc::clone(&flight);
            handles.push(std::thread::spawn(move || {
                flight.run(format!("key-{index}"), || {
                    std::thread::sleep(Duration::from_millis(200));
                    index
                })
            }));
        }
        let mut leaders = 0;
        for handle in handles {
            if handle.join().unwrap().leader {
                leaders += 1;
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(leaders, 4, "each distinct key must run its own load");
        // 4 个不同 key 各睡 200 ms；若被串行化则 ≥ 800 ms。
        assert!(
            elapsed < Duration::from_millis(600),
            "distinct keys must run concurrently; took {elapsed:?}"
        );
    }

    #[test]
    fn leader_failure_is_propagated_identically_to_followers() {
        let flight: Arc<SingleFlight<String, Result<u32, String>>> = Arc::new(SingleFlight::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let flight = Arc::clone(&flight);
            let calls = Arc::clone(&calls);
            handles.push(std::thread::spawn(move || {
                flight
                    .run("failing".to_string(), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        Err("boom".to_string())
                    })
                    .value
            }));
        }
        let results: Vec<Result<u32, String>> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(
            results.iter().all(|r| r == &Err("boom".to_string())),
            "every caller must observe the leader's failure: {results:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a failing leader must not cause followers to retry (stampede)"
        );
    }

    #[test]
    fn the_slot_is_released_so_a_later_call_runs_again() {
        let flight: SingleFlight<String, u32> = SingleFlight::new();
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let calls = Arc::clone(&calls);
            let outcome = flight.run("k".to_string(), || {
                calls.fetch_add(1, Ordering::SeqCst);
                1_u32
            });
            assert!(outcome.leader, "sequential calls must each become leader");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(flight.inflight_keys(), 0, "slots must not leak");
    }

    #[test]
    fn followers_report_how_long_they_waited() {
        let flight: Arc<SingleFlight<String, u32>> = Arc::new(SingleFlight::new());
        let leader = {
            let flight = Arc::clone(&flight);
            std::thread::spawn(move || {
                flight.run("k".to_string(), || {
                    std::thread::sleep(Duration::from_millis(150));
                    5_u32
                })
            })
        };
        std::thread::sleep(Duration::from_millis(30));
        let follower = flight.run("k".to_string(), || panic!("follower must not load"));
        assert!(!follower.leader);
        assert_eq!(follower.value, 5);
        assert!(
            follower.waited_us >= 80_000,
            "follower should report a real wait: {}us",
            follower.waited_us
        );
        leader.join().unwrap();
    }
}

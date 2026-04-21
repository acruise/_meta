/// What external inputs an expression reads. Determines memoization strategy.
///
/// Coeffects are additive: a compound expression's coeffects are the union
/// of its children's. The ordering reflects increasing volatility:
///
/// 1. **Pure** (no flags) — memoize forever
/// 2. **reads_event_data** — memoize per event, discard before next
/// 3. **reads_current_time** — parameterized with interval; result is
///    stable within the interval, must re-evaluate when it elapses
/// 4. **reads_aggregates** — inherently stateful; the expression depends
///    on accumulator state that changes as rows arrive and expire
/// 5. **reads_enrichment** — possibly IO-heavy on cache miss; the
///    enrichment cache may need warming before evaluation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Coeffects {
    pub reads_event_data: bool,
    pub reads_current_time: Option<TimeGranularity>,
    pub reads_aggregates: bool,
    pub reads_enrichment: bool,
}

/// How coarse the time dependency is. When unioning, the finer
/// (smaller) interval wins — the expression is only stable for
/// the shortest interval any sub-expression requires.
///
/// The optional `offset_ms` shifts the bucket boundary. An interval
/// of 3600000 (1 hour) with offset 877000 means "resets every hour
/// at 14m37s past the hour." Offset 0 means epoch-aligned buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeGranularity {
    pub interval_ms: u64,
    pub offset_ms: u64,
}

impl TimeGranularity {
    pub fn new(interval_ms: u64, offset_ms: u64) -> Self {
        Self { interval_ms, offset_ms }
    }

    pub fn finer(self, other: Self) -> Self {
        if self.interval_ms <= other.interval_ms { self } else { other }
    }
}

impl Default for Coeffects {
    fn default() -> Self {
        Self::PURE
    }
}

impl Coeffects {
    pub const PURE: Coeffects = Coeffects {
        reads_event_data: false,
        reads_current_time: None,
        reads_aggregates: false,
        reads_enrichment: false,
    };

    pub fn union(self, other: Coeffects) -> Coeffects {
        Coeffects {
            reads_event_data: self.reads_event_data || other.reads_event_data,
            reads_current_time: match (self.reads_current_time, other.reads_current_time) {
                (None, None) => None,
                (Some(g), None) | (None, Some(g)) => Some(g),
                (Some(a), Some(b)) => Some(a.finer(b)),
            },
            reads_aggregates: self.reads_aggregates || other.reads_aggregates,
            reads_enrichment: self.reads_enrichment || other.reads_enrichment,
        }
    }

    pub fn is_pure(self) -> bool {
        self == Self::PURE
    }

    pub fn all() -> Coeffects {
        Coeffects {
            reads_event_data: true,
            reads_current_time: Some(TimeGranularity::new(0, 0)),
            reads_aggregates: true,
            reads_enrichment: true,
        }
    }

    pub fn event_data() -> Coeffects {
        Coeffects { reads_event_data: true, ..Self::PURE }
    }

    pub fn current_time(interval_ms: u64) -> Coeffects {
        Coeffects { reads_current_time: Some(TimeGranularity::new(interval_ms, 0)), ..Self::PURE }
    }

    pub fn current_time_with_offset(interval_ms: u64, offset_ms: u64) -> Coeffects {
        Coeffects { reads_current_time: Some(TimeGranularity::new(interval_ms, offset_ms)), ..Self::PURE }
    }

    pub fn aggregates() -> Coeffects {
        Coeffects { reads_aggregates: true, ..Self::PURE }
    }

    pub fn enrichment() -> Coeffects {
        Coeffects { reads_enrichment: true, ..Self::PURE }
    }
}

use std::collections::HashSet;

/// What external inputs an expression reads. Determines memoization
/// strategy and evaluation wave ordering (note 10).
///
/// Coeffects are additive: a compound expression's coeffects are the
/// union of its children's. The empty set means pure -- memoize forever.
///
/// Parameterized variants (e.g. `ReadsCurrentTime` with different
/// granularities) are distinct entries in the set. Coalescing
/// (e.g. taking the finest granularity) happens after construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Coeffect {
    /// Reads event field data. Memoize per event, discard before next.
    ReadsEventData,
    /// Reads the current time at a given granularity. Result is stable
    /// within the interval; must re-evaluate when it elapses.
    ReadsCurrentTime(TimeGranularity),
    /// Reads accumulator state. Inherently stateful -- changes as
    /// rows arrive and expire.
    ReadsAggregates,
    /// Reads enrichment data. Possibly IO-heavy on cache miss;
    /// the enrichment cache may need warming before evaluation.
    ReadsEnrichment,
    /// Calls an external UDF. The language determines the trust level:
    /// opaque languages (Rust, Starlark) may have arbitrary side effects,
    /// while sandboxed languages (CEL) are safe but slow.
    CallsExternalUdf(UdfLanguage),
}

/// What language an external UDF is implemented in. Determines the
/// trust/optimization level: opaque languages block constant folding
/// and assume worst-case side effects; sandboxed languages are merely
/// expensive to evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UdfLanguage {
    /// Arbitrary native code (Rust, C FFI). Cannot verify purity.
    Opaque,
    /// CEL expression body. Safe (no IO, no mutation) but slow.
    Cel,
    /// WebAssembly module. Sandboxed and fast.
    Wasm,
}

/// A set of coeffects. Wraps a `HashSet<Coeffect>` with convenience
/// methods for construction and querying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoeffectSet(pub HashSet<Coeffect>);

impl CoeffectSet {
    pub fn new() -> Self {
        Self(HashSet::new())
    }

    pub fn pure() -> Self {
        Self::new()
    }

    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, c: Coeffect) {
        self.0.insert(c);
    }

    pub fn contains(&self, c: &Coeffect) -> bool {
        self.0.contains(c)
    }

    pub fn reads_event_data(&self) -> bool {
        self.0.contains(&Coeffect::ReadsEventData)
    }

    pub fn reads_aggregates(&self) -> bool {
        self.0.contains(&Coeffect::ReadsAggregates)
    }

    pub fn reads_enrichment(&self) -> bool {
        self.0.contains(&Coeffect::ReadsEnrichment)
    }

    pub fn reads_current_time(&self) -> bool {
        self.0.iter().any(|c| matches!(c, Coeffect::ReadsCurrentTime(_)))
    }

    pub fn union(mut self, other: CoeffectSet) -> CoeffectSet {
        self.0.extend(other.0);
        self
    }

    pub fn event_data() -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsEventData);
        s
    }

    pub fn current_time(interval_ms: u64) -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsCurrentTime(TimeGranularity::new(interval_ms, 0)));
        s
    }

    pub fn current_time_with_offset(interval_ms: u64, offset_ms: u64) -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsCurrentTime(TimeGranularity::new(interval_ms, offset_ms)));
        s
    }

    pub fn aggregates() -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsAggregates);
        s
    }

    pub fn enrichment() -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsEnrichment);
        s
    }

    pub fn all() -> Self {
        let mut s = Self::new();
        s.insert(Coeffect::ReadsEventData);
        s.insert(Coeffect::ReadsCurrentTime(TimeGranularity::new(0, 0)));
        s.insert(Coeffect::ReadsAggregates);
        s.insert(Coeffect::ReadsEnrichment);
        s
    }

    /// After construction, coalesce all `ReadsCurrentTime` entries to
    /// the finest (smallest interval) granularity. Call this when you
    /// need a single canonical granularity for timer scheduling.
    pub fn finest_time_granularity(&self) -> Option<TimeGranularity> {
        self.0.iter()
            .filter_map(|c| match c {
                Coeffect::ReadsCurrentTime(g) => Some(*g),
                _ => None,
            })
            .reduce(|a, b| a.finer(b))
    }
}

/// How coarse the time dependency is. When coalescing, the finer
/// (smaller) interval wins -- the expression is only stable for
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

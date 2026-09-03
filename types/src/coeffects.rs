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
///
/// Coeffect *kinds* (the variant tags here) are meant to be declared in
/// the function catalog -- the single source of truth -- as plain string
/// tags; codegen validates that every use names a declared kind. A
/// kind's parameter space is not declared and is open: it is supplied at
/// the use-site, not enumerated in the catalog. `ReadsEventData` /
/// `ReadsAggregates` / `ReadsEnrichment` are nullary; `ReadsCurrentTime`
/// carries a `TimeGranularity` and `CallsExternalUdf` a `UdfLanguage`,
/// neither enumerable in the catalog. The per-entry `coeffects: [...]`
/// list already names declared kinds this way. (Not yet wired through
/// codegen; see `docs/entity-validation-predicates.md`.)
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
    /// Calls an external UDF. The language determines what is known about the
    /// call without anybody attesting to anything; see `UdfLanguage`.
    CallsExternalUdf(UdfLanguage),
}

/// What language an external UDF is implemented in.
///
/// "Opaque" was doing two jobs here and they come apart. A language can be
/// opaque about SIDE EFFECTS, meaning the host cannot tell whether a call
/// touches the world, and it can be opaque about STRUCTURE, meaning the host
/// cannot see what the body computes even though it is confined. Native code is
/// both. Starlark and WebAssembly are only the second, which is a materially
/// better position: they can be memoized, constant-folded, and replayed, and
/// what they cannot do is expose a predicate to a caller that wants to push it
/// down.
///
/// What each supplies before an attestation:
///
/// | language | side effects | determinism | termination | structure visible |
/// |----------|--------------|-------------|-------------|-------------------|
/// | Cel      | none, by language | yes    | yes, not Turing complete | partially, by raising |
/// | Starlark | none, by language | yes, by design | yes, in bounded dialects | no |
/// | Wasm     | per import list   | per import list | needs fuel or epochs | no |
/// | Native   | unknown           | unknown     | unknown     | no |
///
/// The realistic candidates for user-supplied computation today are CEL,
/// Starlark, and WebAssembly. `Native` is how a host's own builtins are written
/// and is not a path user code is expected to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UdfLanguage {
    /// Arbitrary native code (Rust, C FFI). Nothing is verifiable.
    ///
    /// Named `Opaque` rather than `Native` for compatibility with existing
    /// call sites; it means "nothing known", not merely "structurally
    /// unreadable", which is what the other entries below are.
    Opaque,
    /// CEL expression body. No IO and no mutation by construction, and
    /// guaranteed to terminate because the language is deliberately not Turing
    /// complete. Slow relative to a compiled kernel, and the only entry here
    /// whose body can be partially raised against the function catalog, so a
    /// consumer can recover some of its structure rather than none.
    Cel,
    /// Starlark function body. Deterministic and hermetic by design, since the
    /// language exists to make builds reproducible, and bounded in the dialects
    /// that forbid unbounded recursion. Structurally opaque: nothing in a body
    /// tells a consumer what it computes.
    Starlark,
    /// WebAssembly module. Sandboxed and fast, with purity and determinism
    /// following from the import list rather than from a claim. Termination
    /// does NOT follow: WebAssembly is Turing complete, so a module needs fuel
    /// metering or epoch interruption, and a host that omits it has a denial of
    /// service rather than a slow call.
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

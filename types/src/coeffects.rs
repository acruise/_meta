use std::collections::HashSet;
use std::convert::Infallible;
use std::fmt::Debug;
use std::hash::Hash;

/// A host-defined coeffect kind, carried in [`Coeffect::Ext`]. A consumer
/// that needs kinds this crate does not know about (reads of its own
/// state stores, caches, model outputs, ...) declares a plain enum and
/// implements this marker for it. `Infallible` is the "no extensions"
/// domain and the default.
pub trait CoeffectExt: Debug + Clone + Eq + Hash {}

impl CoeffectExt for Infallible {}

/// What external inputs an expression reads. Determines memoization
/// strategy and evaluation wave ordering.
///
/// Coeffects are additive: a compound expression's coeffects are the
/// union of its children's. The empty set means pure -- memoize forever.
///
/// Parameterized variants (e.g. `ReadsCurrentTime` with different
/// granularities) are distinct entries in the set. Coalescing
/// (e.g. taking the finest granularity) happens after construction.
///
/// The kinds here are the ones this crate itself produces: the shared
/// catalog names them as plain string tags (`reads_event_data`,
/// `reads_current_time`) and codegen emits them. Kinds specific to a
/// consumer go in `Ext(X)` with a host-defined `X`; generated code
/// produces `Coeffect<Infallible>`, which a host lifts with
/// [`CoeffectSet::widen`]. A kind's parameter space is open and supplied
/// at the use-site, not enumerated in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Coeffect<X: CoeffectExt = Infallible> {
    /// Reads event field data. Memoize per event, discard before next.
    ReadsEventData,
    /// Reads the current time at a given granularity. Result is stable
    /// within the interval; must re-evaluate when it elapses.
    ReadsCurrentTime(TimeGranularity),
    /// Calls an external UDF. The language determines the trust level:
    /// opaque languages (Rust, Starlark) may have arbitrary side effects,
    /// while sandboxed languages (CEL) are safe but slow.
    CallsExternalUdf(UdfLanguage),
    /// Conservative top: may read anything, including extension kinds
    /// this crate cannot name. Used for expressions not yet analyzed
    /// (e.g. an unlowered CEL fallback). Distinct from
    /// `CallsExternalUdf(Opaque)`: an unanalyzed expression may turn out
    /// pure once lowered, an opaque call never does. Every
    /// [`CoeffectSet::may_read`] query answers true when this is present.
    Unknown,
    /// A host-defined kind.
    Ext(X),
}

impl<X: CoeffectExt> Coeffect<X> {
    /// Re-key the extension domain, leaving core kinds unchanged.
    pub fn map_ext<Y: CoeffectExt>(self, f: impl FnOnce(X) -> Y) -> Coeffect<Y> {
        match self {
            Coeffect::ReadsEventData => Coeffect::ReadsEventData,
            Coeffect::ReadsCurrentTime(g) => Coeffect::ReadsCurrentTime(g),
            Coeffect::CallsExternalUdf(l) => Coeffect::CallsExternalUdf(l),
            Coeffect::Unknown => Coeffect::Unknown,
            Coeffect::Ext(x) => Coeffect::Ext(f(x)),
        }
    }
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

/// A set of coeffects. Wraps a `HashSet<Coeffect<X>>` with convenience
/// methods for construction and querying.
///
/// Two kinds of query: [`contains`](Self::contains) is exact membership;
/// [`may_read`](Self::may_read) is the conservative question ("could this
/// expression read X?"), which also answers true when `Unknown` is
/// present. Memoization and scheduling decisions should use `may_read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoeffectSet<X: CoeffectExt = Infallible>(pub HashSet<Coeffect<X>>);

impl<X: CoeffectExt> Default for CoeffectSet<X> {
    fn default() -> Self {
        Self(HashSet::new())
    }
}

impl<X: CoeffectExt> CoeffectSet<X> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pure() -> Self {
        Self::new()
    }

    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, c: Coeffect<X>) {
        self.0.insert(c);
    }

    /// Exact membership.
    pub fn contains(&self, c: &Coeffect<X>) -> bool {
        self.0.contains(c)
    }

    pub fn is_unknown(&self) -> bool {
        self.0.contains(&Coeffect::Unknown)
    }

    /// Conservative membership: true if `c` is present or the set is
    /// `Unknown`.
    pub fn may_read(&self, c: &Coeffect<X>) -> bool {
        self.is_unknown() || self.contains(c)
    }

    pub fn reads_event_data(&self) -> bool {
        self.may_read(&Coeffect::ReadsEventData)
    }

    pub fn reads_current_time(&self) -> bool {
        self.is_unknown() || self.0.iter().any(|c| matches!(c, Coeffect::ReadsCurrentTime(_)))
    }

    /// The extension kinds in this set. Does not account for `Unknown`;
    /// check [`is_unknown`](Self::is_unknown) separately.
    pub fn ext(&self) -> impl Iterator<Item = &X> {
        self.0.iter().filter_map(|c| match c {
            Coeffect::Ext(x) => Some(x),
            _ => None,
        })
    }

    pub fn union(mut self, other: CoeffectSet<X>) -> CoeffectSet<X> {
        self.0.extend(other.0);
        self
    }

    /// Re-key the extension domain of every entry.
    pub fn map_ext<Y: CoeffectExt>(self, f: impl Fn(X) -> Y) -> CoeffectSet<Y> {
        CoeffectSet(self.0.into_iter().map(|c| c.map_ext(&f)).collect())
    }

    pub fn event_data() -> Self {
        Self::of(Coeffect::ReadsEventData)
    }

    pub fn current_time(interval_ms: u64) -> Self {
        Self::of(Coeffect::ReadsCurrentTime(TimeGranularity::new(interval_ms, 0)))
    }

    pub fn current_time_with_offset(interval_ms: u64, offset_ms: u64) -> Self {
        Self::of(Coeffect::ReadsCurrentTime(TimeGranularity::new(interval_ms, offset_ms)))
    }

    pub fn external_udf(lang: UdfLanguage) -> Self {
        Self::of(Coeffect::CallsExternalUdf(lang))
    }

    /// The conservative top: may read anything.
    pub fn unknown() -> Self {
        Self::of(Coeffect::Unknown)
    }

    pub fn ext_kind(x: X) -> Self {
        Self::of(Coeffect::Ext(x))
    }

    fn of(c: Coeffect<X>) -> Self {
        let mut s = Self::new();
        s.insert(c);
        s
    }

    /// After construction, coalesce all `ReadsCurrentTime` entries to
    /// the finest (smallest interval) granularity. Call this when you
    /// need a single canonical granularity for timer scheduling. An
    /// `Unknown` set answers the finest possible granularity (0 ms).
    pub fn finest_time_granularity(&self) -> Option<TimeGranularity> {
        if self.is_unknown() {
            return Some(TimeGranularity::new(0, 0));
        }
        self.0.iter()
            .filter_map(|c| match c {
                Coeffect::ReadsCurrentTime(g) => Some(*g),
                _ => None,
            })
            .reduce(|a, b| a.finer(b))
    }
}

impl CoeffectSet<Infallible> {
    /// Lift a core-only set (e.g. from generated `LogExpr` analysis) into
    /// a host's extension domain. Total: `Infallible` has no values.
    pub fn widen<Y: CoeffectExt>(self) -> CoeffectSet<Y> {
        self.map_ext(|n| match n {})
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum HostKind {
        ReadsCache,
    }
    impl CoeffectExt for HostKind {}

    #[test]
    fn widen_preserves_core_kinds() {
        let core = CoeffectSet::event_data().union(CoeffectSet::current_time(60_000));
        let wide: CoeffectSet<HostKind> = core.widen();
        assert!(wide.reads_event_data());
        assert_eq!(wide.finest_time_granularity(), Some(TimeGranularity::new(60_000, 0)));
        assert_eq!(wide.ext().count(), 0);
    }

    #[test]
    fn ext_kinds_union_with_core() {
        let s = CoeffectSet::<HostKind>::event_data()
            .union(CoeffectSet::ext_kind(HostKind::ReadsCache));
        assert!(s.contains(&Coeffect::Ext(HostKind::ReadsCache)));
        assert_eq!(s.ext().collect::<Vec<_>>(), vec![&HostKind::ReadsCache]);
        assert!(!s.reads_current_time());
    }

    #[test]
    fn unknown_answers_every_may_read() {
        let s = CoeffectSet::<HostKind>::unknown();
        assert!(!s.is_pure());
        assert!(s.may_read(&Coeffect::Ext(HostKind::ReadsCache)));
        assert!(!s.contains(&Coeffect::Ext(HostKind::ReadsCache)));
        assert!(s.reads_event_data());
        assert!(s.reads_current_time());
        assert_eq!(s.finest_time_granularity(), Some(TimeGranularity::new(0, 0)));
    }
}

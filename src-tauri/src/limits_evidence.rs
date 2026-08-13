//! Limit Evidence Intervals — derived, never persisted (ADR-0024).
//!
//! A Limit Evidence Interval is a positive movement between two comparable Limit
//! Readings plus the Usage Records that landed between them. This module turns
//! stored Readings and the current Ledger into those intervals, and counts by
//! reason everything it refused. Nothing here is written back: a later Record,
//! a corrected coverage fact, or a supersession changes the answer simply by
//! being read again.
//!
//! What it deliberately does not do: form runs, choose epoch representatives,
//! or compute a ratio. Those are the estimator's, one ticket along.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};

use crate::queries;
use crate::types::{LimitReading, ModelScope, ReadingProvenance};

/// Why evidence was refused. The backend returns codes and values, never
/// localized prose (spec: "Reason codes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonCode {
    MissingAccountIdentity,
    MissingPlanIdentity,
    MissingMeteringRegime,
    MissingLimitIdentity,
    MissingModelScope,
    UnprovenSourceCompleteness,
    AmbiguousReadingOrder,
    IdentityChange,
    ResetBoundary,
    PercentageDecrease,
    PercentageSaturation,
    ZeroLocalUsage,
    KnownExternalActivity,
    UnattributedModelUsage,
}

impl ReasonCode {
    /// The wire spelling the specification fixes.
    pub fn code(self) -> &'static str {
        match self {
            ReasonCode::MissingAccountIdentity => "missing-account-identity",
            ReasonCode::MissingPlanIdentity => "missing-plan-identity",
            ReasonCode::MissingMeteringRegime => "missing-metering-regime",
            ReasonCode::MissingLimitIdentity => "missing-limit-identity",
            ReasonCode::MissingModelScope => "missing-model-scope",
            ReasonCode::UnprovenSourceCompleteness => "unproven-source-completeness",
            ReasonCode::AmbiguousReadingOrder => "ambiguous-reading-order",
            ReasonCode::IdentityChange => "identity-change",
            ReasonCode::ResetBoundary => "reset-boundary",
            ReasonCode::PercentageDecrease => "percentage-decrease",
            ReasonCode::PercentageSaturation => "percentage-saturation",
            ReasonCode::ZeroLocalUsage => "zero-local-usage",
            ReasonCode::KnownExternalActivity => "known-external-activity",
            ReasonCode::UnattributedModelUsage => "unattributed-model-usage",
        }
    }
}

/// The identity comparable Readings share. Every field is proven or the Reading
/// is not evidence at all — an unknown is never a wildcard, so there is no
/// `Option` in here by design.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeriesKey {
    pub source: String,
    pub account_id: String,
    pub plan: String,
    pub metering_regime: String,
    pub limit_id: String,
    /// The stored grammar, so scoping the same Models is one string.
    pub model_scope: String,
}

impl SeriesKey {
    /// The Series a Reading belongs to, or the first fact it could not prove.
    fn of(reading: &LimitReading) -> Result<SeriesKey, ReasonCode> {
        let p = &reading.provenance;
        Ok(SeriesKey {
            source: reading.source.clone(),
            account_id: p.account_id.clone().ok_or(ReasonCode::MissingAccountIdentity)?,
            plan: reading.plan.clone().ok_or(ReasonCode::MissingPlanIdentity)?,
            metering_regime: p
                .metering_regime
                .clone()
                .ok_or(ReasonCode::MissingMeteringRegime)?,
            limit_id: p.limit_id.clone().ok_or(ReasonCode::MissingLimitIdentity)?,
            model_scope: p
                .model_scope
                .as_ref()
                .and_then(ModelScope::stored)
                .ok_or(ReasonCode::MissingModelScope)?,
        })
    }
}

/// A Usage Record reduced to what membership and canonical tokens need — the
/// spec's "matching Usage Records", the ones an interval may count.
/// It names its own Source and account because one read serves every Partition,
/// and a Partition must never count tokens that were never its own.
#[derive(Debug, Clone)]
pub struct MatchingRecord {
    pub source: String,
    /// Proven account identity. A Record that cannot name its account cannot
    /// participate at all, so it never becomes one of these.
    pub account_id: String,
    pub timestamp: i64,
    /// None is Unattributed Usage — never a sentinel Model.
    pub model: Option<String>,
    /// Canonical tokens: Input + Output + Cache Read + both Cache Writes,
    /// counted once each. Reasoning and context fields are classifications of
    /// those, so adding them would double-count.
    pub tokens: i64,
}

/// A positive movement between two comparable Readings, and the canonical tokens
/// that landed inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    /// Displayed percentages: `round(clamp(used_pct, 0, 100))`, the same figure
    /// the row shows.
    pub from_pct: i64,
    pub to_pct: i64,
    pub tokens: i64,
    /// The exclusive lower bound and inclusive upper bound of membership.
    pub t0: i64,
    pub t1: i64,
}

impl Interval {
    pub fn movement(&self) -> i64 {
        self.to_pct - self.from_pct
    }
}

/// One Partition's eligible intervals, in observation order.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionEvidence {
    pub series: SeriesKey,
    /// The reset instant the Partition is identified by — the earliest stamp of
    /// the jitter band the Readings agreed on.
    pub epoch: i64,
    pub intervals: Vec<Interval>,
}

/// Everything derivable from one consistent read, and everything refused.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    pub partitions: Vec<PartitionEvidence>,
    /// Bounded counts, so the normal payload stays small; the diagnostic path is
    /// what expands them to contributors.
    pub rejections: BTreeMap<ReasonCode, usize>,
}

impl Evidence {
    fn refuse(&mut self, reason: ReasonCode) {
        *self.rejections.entry(reason).or_insert(0) += 1;
    }
}

/// A percentage that is not a number at all. Not a readiness state: the
/// specification calls this an invariant failure, so it travels the technical
/// error path rather than masquerading as evidence nobody has yet.
#[derive(Debug, Clone, PartialEq)]
pub struct NonFinitePercentage {
    pub source: String,
    pub observed_at: i64,
}

/// The displayed percentage — the one figure movement, saturation and the
/// visible numeral all use.
fn displayed(reading: &LimitReading) -> Result<i64, NonFinitePercentage> {
    if !reading.used_pct.is_finite() {
        return Err(NonFinitePercentage {
            source: reading.source.clone(),
            observed_at: reading.observed_at,
        });
    }
    Ok(reading.used_pct.clamp(0.0, 100.0).round() as i64)
}

/// Which Partition a Reading belongs to, once its Series and its epoch are both
/// settled. `None` is a Reading that proves too little to belong anywhere.
type Placement = Option<(SeriesKey, i64)>;

/// Derive every eligible interval from stored Readings and the current Ledger.
///
/// `usage` is every Record that can participate at all — one that names its own
/// account — for any of these Readings; each Partition takes only the Records
/// whose Source and account are its own.
pub fn derive(
    readings: &[LimitReading],
    usage: &[MatchingRecord],
) -> Result<Evidence, NonFinitePercentage> {
    let mut evidence = Evidence::default();
    let mut intervals: BTreeMap<(SeriesKey, i64), Vec<Interval>> = BTreeMap::new();

    // One timeline per Limit as the card addresses it. `window_key` is the only
    // address every Reading has, provable or not, and it is used here for that
    // alone — to notice that something happened between two Readings — never as
    // identity, which is the Series' job.
    let mut timelines: BTreeMap<(&str, &str), Vec<&LimitReading>> = BTreeMap::new();
    for reading in readings {
        timelines
            .entry((reading.source.as_str(), reading.window_key.as_str()))
            .or_default()
            .push(reading);
    }

    for mut timeline in timelines.into_values() {
        // Observation order only. `source_order` could break a tie, but it is a
        // byte offset with no Artifact identity beside it, so it cannot prove
        // that two Readings of one instant are even comparable — and a run that
        // trusted it to sort would be trusting what the interval rule below
        // refuses to trust to bound.
        timeline.sort_by_key(|r| r.observed_at);
        let placed = place(&timeline, &mut evidence)?;
        walk(&timeline, &placed, usage, &mut evidence, &mut intervals)?;
    }

    for ((series, epoch), intervals) in intervals {
        if !intervals.is_empty() {
            evidence.partitions.push(PartitionEvidence { series, epoch, intervals });
        }
    }
    Ok(evidence)
}

/// Place each Reading of one timeline in its Partition.
///
/// A reset stamp jitters inside what is plainly one window (#104), so stamps
/// within one band are one epoch — but only while banding them introduces no
/// decrease. A fall inside a band is the signature of a reset the jitter hid,
/// and the specification rejects an ambiguous grouping rather than guessing at
/// it, so the band ends there and a new epoch begins. Two Readings sharing an
/// exact stamp are one epoch whatever their percentages do: that is not a
/// grouping question, and the decrease between them is the walk's to refuse.
fn place(
    timeline: &[&LimitReading],
    evidence: &mut Evidence,
) -> Result<Vec<Placement>, NonFinitePercentage> {
    // Per Series: the epoch being accumulated and the highest percentage in it.
    let mut open: BTreeMap<SeriesKey, (i64, i64)> = BTreeMap::new();
    let mut placed = Vec::with_capacity(timeline.len());

    for reading in timeline {
        let series = match SeriesKey::of(reading) {
            Ok(series) => series,
            Err(missing) => {
                evidence.refuse(missing);
                placed.push(None);
                continue;
            }
        };
        let pct = displayed(reading)?;
        let epoch = match open.get(&series) {
            Some(&(epoch, highest)) => {
                let same_stamp = reading.resets_at == epoch;
                let banded = (reading.resets_at - epoch).abs() <= queries::EPOCH_JITTER_SECS;
                if same_stamp || (banded && pct >= highest) {
                    epoch
                } else {
                    reading.resets_at
                }
            }
            None => reading.resets_at,
        };
        let highest = match open.get(&series) {
            Some(&(open_epoch, highest)) if open_epoch == epoch => highest.max(pct),
            _ => pct,
        };
        open.insert(series.clone(), (epoch, highest));
        placed.push(Some((series, epoch)));
    }
    Ok(placed)
}

/// Walk one Limit's whole timeline, emitting an interval at each positive
/// movement the evidence supports and counting every refusal.
///
/// Repeated Readings at one percentage do not make zero-movement intervals:
/// tokens keep accumulating from the last anchor until the percentage moves.
/// Anything that interrupts the run ends that accumulation — a decrease, a
/// saturation, a fact that spoils the stretch, and equally a Reading of another
/// Partition or of none at all standing between the anchor and the movement.
/// That last case is why the walk sees the whole timeline rather than one
/// Partition's Readings: a run cannot step over a Reading that says the account,
/// the meter, the Limit or the window changed in the middle of it.
fn walk(
    timeline: &[&LimitReading],
    placed: &[Placement],
    usage: &[MatchingRecord],
    evidence: &mut Evidence,
    out: &mut BTreeMap<(SeriesKey, i64), Vec<Interval>>,
) -> Result<(), NonFinitePercentage> {
    let mut anchor: Option<(&LimitReading, &(SeriesKey, i64), ModelScope)> = None;
    // A fact carried by a Reading the walk passed over still sits inside the
    // movement being accumulated. The anchor's own does not: it describes the
    // stretch that ended there, which is the previous interval's business.
    let mut spoiled: Option<ReasonCode> = None;

    for (reading, placement) in timeline.iter().zip(placed) {
        let Some(here) = placement else {
            // A Reading that proves too little to be placed is still a Reading
            // of this Limit, and a run cannot see past it.
            anchor = None;
            spoiled = None;
            continue;
        };
        let Some((from, was, scope)) = &anchor else {
            anchor = anchored(reading, here, evidence);
            spoiled = None;
            continue;
        };

        if *was != here {
            evidence.refuse(if was.0 == here.0 {
                ReasonCode::ResetBoundary
            } else {
                ReasonCode::IdentityChange
            });
            anchor = anchored(reading, here, evidence);
            spoiled = None;
            continue;
        }

        let (from_pct, to_pct) = (displayed(from)?, displayed(reading)?);
        if reading.provenance.external_activity.is_some() {
            spoiled = Some(ReasonCode::KnownExternalActivity);
        }
        if to_pct == from_pct {
            continue;
        }
        if to_pct < from_pct {
            evidence.refuse(ReasonCode::PercentageDecrease);
            anchor = anchored(reading, here, evidence);
            spoiled = None;
            continue;
        }
        if to_pct >= 100 {
            // Saturation: the window is full, and what filled the last points of
            // it cannot be told from what would have overflowed.
            evidence.refuse(ReasonCode::PercentageSaturation);
            anchor = anchored(reading, here, evidence);
            spoiled = None;
            continue;
        }

        let candidate = spoiled.map_or_else(
            || interval(from, reading, from_pct, to_pct, usage, &here.0, scope),
            Err,
        );
        match candidate {
            Ok(interval) => out.entry(here.clone()).or_default().push(interval),
            Err(reason) => evidence.refuse(reason),
        }
        anchor = anchored(reading, here, evidence);
        spoiled = None;
    }
    Ok(())
}

/// Anchor a run at this Reading, reading its Model scope once for every
/// candidate the run will weigh. The stored scope came from `ModelScope::stored`
/// and parses; a Reading whose scope does not is one whose Series should never
/// have been built, and it anchors nothing.
fn anchored<'a>(
    reading: &'a LimitReading,
    here: &'a (SeriesKey, i64),
    evidence: &mut Evidence,
) -> Option<(&'a LimitReading, &'a (SeriesKey, i64), ModelScope)> {
    match ModelScope::parse(&here.0.model_scope) {
        Some(scope) => Some((reading, here, scope)),
        None => {
            evidence.refuse(ReasonCode::MissingModelScope);
            None
        }
    }
}

/// One candidate interval, or the first fact that rejects it.
fn interval(
    from: &LimitReading,
    to: &LimitReading,
    from_pct: i64,
    to_pct: i64,
    usage: &[MatchingRecord],
    series: &SeriesKey,
    scope: &ModelScope,
) -> Result<Interval, ReasonCode> {
    let (t0, t1) = (from.observed_at, to.observed_at);

    // Two Readings of one instant bound nothing. `source_order` is the only
    // thing that could order them, and it is a byte offset whose column carries
    // no Artifact identity — so it cannot prove the two are even from the one
    // Artifact whose offsets compare.
    if t0 == t1 {
        return Err(ReasonCode::AmbiguousReadingOrder);
    }

    // Coverage has to reach back past the earlier anchor: local capture of this
    // Source and account must be proven unbroken across the whole stretch, not
    // merely un-erroring today.
    match to.provenance.covered_from {
        Some(covered_from) if covered_from <= t0 => {}
        _ => return Err(ReasonCode::UnprovenSourceCompleteness),
    }

    // The scope came from `ModelScope::stored`, so it parses; the Series would
    // not exist otherwise.
    let members: Vec<&MatchingRecord> = usage
        .iter()
        .filter(|u| {
            u.source == series.source
                && u.account_id == series.account_id
                && t0 < u.timestamp
                && u.timestamp <= t1
        })
        .collect();

    let tokens = match scope {
        // Source-wide: every Record of this Source and account counts, and
        // Unattributed Usage is Usage.
        ModelScope::All => members.iter().map(|u| u.tokens).sum(),
        ModelScope::Models(models) => {
            // A Record that might be one of the scoped Models, but cannot say,
            // invalidates the interval rather than being guessed at or ignored.
            if members.iter().any(|u| u.model.is_none()) {
                return Err(ReasonCode::UnattributedModelUsage);
            }
            members
                .iter()
                .filter(|u| u.model.as_ref().is_some_and(|m| models.contains(m)))
                .map(|u| u.tokens)
                .sum()
        }
    };

    // Movement with nothing local behind it is detected non-local activity, not
    // a conversion of zero tokens.
    if tokens == 0 {
        return Err(ReasonCode::ZeroLocalUsage);
    }

    Ok(Interval { from_pct, to_pct, tokens, t0, t1 })
}

/// Stored Limit Readings observed at or after `since`, with the provenance that
/// decides whether each is evidence.
///
/// The horizon is the caller's: evidence has one (ADR-0024 asks for a bounded
/// read, and the readiness policy is what sets its length), and this table grows
/// by a row per observation for as long as the app runs, so reading all of it
/// would cost more every week. The Partition a Reading belongs to cannot be
/// known without its provenance, so the columns come along and the derivation
/// sorts them out.
pub fn stored_readings(conn: &Connection, since: i64) -> rusqlite::Result<Vec<LimitReading>> {
    let mut stmt = conn.prepare(
        "SELECT source, window_key, window_minutes, used_pct, resets_at, observed_at, via, \
                plan, account_id, metering_regime, limit_id, model_scope, source_order, \
                covered_from, external_activity \
         FROM limit_readings WHERE observed_at >= ?1 \
         ORDER BY source, window_key, resets_at, observed_at",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok(LimitReading {
            source: r.get(0)?,
            window_key: r.get(1)?,
            window_minutes: r.get(2)?,
            used_pct: r.get(3)?,
            resets_at: r.get(4)?,
            observed_at: r.get(5)?,
            via: r.get(6)?,
            plan: r.get(7)?,
            provenance: ReadingProvenance {
                account_id: r.get(8)?,
                metering_regime: r.get(9)?,
                limit_id: r.get(10)?,
                model_scope: r
                    .get::<_, Option<String>>(11)?
                    .and_then(|s| ModelScope::parse(&s)),
                source_order: r.get(12)?,
                covered_from: r.get(13)?,
                external_activity: r.get(14)?,
            },
        })
    })?;
    rows.collect()
}

/// The Usage Records those Readings could ever be paired with: the ones naming
/// an account, for each Source and account the Readings themselves prove, across
/// the span they cover. One indexed range scan per Source and account, never one
/// per candidate interval and never the whole Ledger.
///
/// A Record with no account identity is not selected at all — it cannot
/// participate, and leaving it out here is the same answer as excluding it
/// later, reached without carrying it.
pub fn matching_usage(
    conn: &Connection,
    readings: &[LimitReading],
) -> rusqlite::Result<Vec<MatchingRecord>> {
    // Source and account, with the span of the Readings that named them.
    let mut spans: BTreeMap<(String, String), (i64, i64)> = BTreeMap::new();
    for reading in readings {
        let Some(account_id) = reading.provenance.account_id.clone() else { continue };
        let span = spans
            .entry((reading.source.clone(), account_id))
            .or_insert((reading.observed_at, reading.observed_at));
        span.0 = span.0.min(reading.observed_at);
        span.1 = span.1.max(reading.observed_at);
    }

    let mut stmt = conn.prepare(
        "SELECT timestamp, model, \
                input_tokens + output_tokens + cache_read_tokens \
                  + cache_write_5m_tokens + cache_write_1h_tokens \
         FROM events \
         WHERE source = ?1 AND account_id = ?2 AND timestamp > ?3 AND timestamp <= ?4",
    )?;
    let mut out = Vec::new();
    for ((source, account_id), (from, through)) in spans {
        // `>` on the low bound: the earliest Reading is an anchor, and nothing at
        // or before it can belong to an interval that starts there.
        let rows = stmt.query_map(params![source, account_id, from, through], |r| {
            Ok(MatchingRecord {
                source: source.clone(),
                account_id: account_id.clone(),
                timestamp: r.get(0)?,
                model: r.get(1)?,
                tokens: r.get(2)?,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// The whole derivation in one consistent read: what the Ledger and the stored
/// Readings prove right now, and what they refused.
pub fn evidence(
    conn: &Connection,
    since: i64,
) -> rusqlite::Result<Result<Evidence, NonFinitePercentage>> {
    let readings = stored_readings(conn, since)?;
    let usage = matching_usage(conn, &readings)?;
    Ok(derive(&readings, &usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReadingProvenance;

    const T0: i64 = 1_786_800_000;
    const RESET: i64 = 1_786_900_000;

    /// A Reading that proves everything, so a test can spoil exactly one fact.
    fn reading(used_pct: f64, observed_at: i64) -> LimitReading {
        LimitReading {
            source: "codex".to_string(),
            window_key: "w10080".to_string(),
            window_minutes: Some(10080),
            used_pct,
            resets_at: RESET,
            observed_at,
            via: "logs".to_string(),
            plan: Some("plus".to_string()),
            provenance: ReadingProvenance {
                account_id: Some("acct-a".to_string()),
                metering_regime: Some("codex:rate_limits".to_string()),
                limit_id: Some("codex:w10080".to_string()),
                model_scope: Some(ModelScope::All),
                source_order: Some(observed_at),
                // Proven back to before any fixture's first Reading.
                covered_from: Some(T0 - 1_000),
                external_activity: None,
            },
        }
    }

    fn record(timestamp: i64, model: Option<&str>, tokens: i64) -> MatchingRecord {
        MatchingRecord {
            source: "codex".to_string(),
            account_id: "acct-a".to_string(),
            timestamp,
            model: model.map(str::to_string),
            tokens,
        }
    }

    fn usage(timestamp: i64, tokens: i64) -> MatchingRecord {
        record(timestamp, Some("gpt-5.4-codex"), tokens)
    }

    fn unattributed(timestamp: i64, tokens: i64) -> MatchingRecord {
        record(timestamp, None, tokens)
    }

    /// Every fixture but the invariant one is well-formed evidence.
    fn derived(readings: &[LimitReading], usage: &[MatchingRecord]) -> Evidence {
        derive(readings, usage).expect("fixture percentages are finite")
    }

    fn only_intervals(evidence: &Evidence) -> Vec<Interval> {
        evidence.partitions.iter().flat_map(|p| p.intervals.clone()).collect()
    }

    fn refused(evidence: &Evidence, reason: ReasonCode) -> usize {
        evidence.rejections.get(&reason).copied().unwrap_or(0)
    }

    #[test]
    fn a_positive_movement_carries_the_tokens_that_landed_inside_it() {
        let evidence = derived(
            &[reading(40.0, T0), reading(50.0, T0 + 600)],
            &[usage(T0 + 100, 700), usage(T0 + 200, 300)],
        );
        assert_eq!(
            only_intervals(&evidence),
            vec![Interval { from_pct: 40, to_pct: 50, tokens: 1_000, t0: T0, t1: T0 + 600 }],
        );
        assert_eq!(only_intervals(&evidence)[0].movement(), 10);
    }

    #[test]
    fn membership_excludes_the_earlier_anchor_and_includes_the_later_one() {
        // The rule is `t0 < timestamp <= t1`, which is what lets a Codex delta
        // emitted in the same snapshot as the later Reading belong to it.
        let evidence = derived(
            &[reading(40.0, T0), reading(50.0, T0 + 600)],
            &[usage(T0, 999), usage(T0 + 600, 500), usage(T0 + 601, 999)],
        );
        assert_eq!(only_intervals(&evidence)[0].tokens, 500);
    }

    #[test]
    fn a_repeated_percentage_accumulates_to_the_next_movement() {
        // Three Readings at 40, then one at 42: one interval, not two, and it
        // carries everything since the first anchor. A zero-movement interval
        // would be a zero-ratio one, which the estimator must never see.
        let evidence = derived(
            &[
                reading(40.0, T0),
                reading(40.0, T0 + 100),
                reading(40.0, T0 + 200),
                reading(42.0, T0 + 300),
            ],
            &[usage(T0 + 50, 100), usage(T0 + 150, 100), usage(T0 + 250, 100)],
        );
        assert_eq!(
            only_intervals(&evidence),
            vec![Interval { from_pct: 40, to_pct: 42, tokens: 300, t0: T0, t1: T0 + 300 }],
        );
    }

    #[test]
    fn source_wide_membership_counts_unattributed_usage() {
        let evidence = derived(
            &[reading(40.0, T0), reading(50.0, T0 + 600)],
            &[usage(T0 + 100, 400), unattributed(T0 + 200, 600)],
        );
        assert_eq!(only_intervals(&evidence)[0].tokens, 1_000);
    }

    #[test]
    fn model_scoped_membership_takes_the_mapped_models_and_no_others() {
        let scoped = |r: &mut LimitReading| {
            r.provenance.model_scope =
                Some(ModelScope::Models(vec!["claude-opus-4-5".to_string()]));
        };
        let mut from = reading(40.0, T0);
        let mut to = reading(50.0, T0 + 600);
        scoped(&mut from);
        scoped(&mut to);
        let model = |name: &str, tokens: i64| record(T0 + 100, Some(name), tokens);
        let evidence = derived(
            &[from.clone(), to.clone()],
            &[model("claude-opus-4-5", 800), model("claude-sonnet-4-5", 500)],
        );
        assert_eq!(only_intervals(&evidence)[0].tokens, 800, "a known nonmatch is not in scope");

        // A Record that might be one of the scoped Models and cannot say
        // invalidates the interval rather than being guessed at or ignored.
        let evidence = derived(&[from, to], &[model("claude-opus-4-5", 800), unattributed(T0 + 200, 1)]);
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::UnattributedModelUsage), 1);
    }

    #[test]
    fn a_partition_counts_only_the_records_that_are_its_own() {
        let mut theirs = record(T0 + 100, Some("gpt-5.4-codex"), 9_000);
        theirs.account_id = "acct-b".to_string();
        let mut elsewhere = record(T0 + 200, Some("claude-opus-4-5"), 9_000);
        elsewhere.source = "claude".to_string();
        let evidence = derived(
            &[reading(40.0, T0), reading(50.0, T0 + 600)],
            &[usage(T0 + 300, 700), theirs, elsewhere],
        );
        assert_eq!(only_intervals(&evidence)[0].tokens, 700);
    }

    #[test]
    fn every_identity_fact_is_required_and_names_itself_when_missing() {
        type Spoil = fn(&mut LimitReading);
        let spoil: [(Spoil, ReasonCode); 5] = [
            (|r| r.provenance.account_id = None, ReasonCode::MissingAccountIdentity),
            (|r| r.plan = None, ReasonCode::MissingPlanIdentity),
            (|r| r.provenance.metering_regime = None, ReasonCode::MissingMeteringRegime),
            (|r| r.provenance.limit_id = None, ReasonCode::MissingLimitIdentity),
            (|r| r.provenance.model_scope = None, ReasonCode::MissingModelScope),
        ];
        for (spoil, reason) in spoil {
            let mut from = reading(40.0, T0);
            spoil(&mut from);
            let evidence = derived(&[from, reading(50.0, T0 + 600)], &[usage(T0 + 100, 1_000)]);
            assert!(only_intervals(&evidence).is_empty(), "{}", reason.code());
            assert_eq!(refused(&evidence, reason), 1, "{}", reason.code());
        }
    }

    #[test]
    fn a_change_of_identity_separates_evidence_rather_than_joining_it() {
        // Same window, same epoch, different account: two Partitions, and the
        // movement between them is nobody's interval.
        let mut other = reading(50.0, T0 + 600);
        other.provenance.account_id = Some("acct-b".to_string());
        let evidence = derived(&[reading(40.0, T0), other], &[usage(T0 + 100, 1_000)]);
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(evidence.partitions.len(), 0, "neither Partition has a movement of its own");
    }

    #[test]
    fn a_reset_starts_a_new_partition_and_bounds_nothing_across_it() {
        let mut next_epoch = reading(5.0, T0 + 600);
        next_epoch.resets_at = RESET + 604_800;
        let evidence = derived(&[reading(40.0, T0), next_epoch], &[usage(T0 + 100, 1_000)]);
        assert!(only_intervals(&evidence).is_empty());
    }

    #[test]
    fn reset_jitter_inside_the_band_is_one_epoch() {
        // The server recomputes the stamp per response and it wobbles by up to
        // ±117s (#104); that is one window, not two.
        let mut jittered = reading(50.0, T0 + 600);
        jittered.resets_at = RESET + 117;
        let evidence = derived(&[reading(40.0, T0), jittered], &[usage(T0 + 100, 1_000)]);
        assert_eq!(only_intervals(&evidence).len(), 1);
    }

    #[test]
    fn a_decrease_and_a_saturation_each_end_the_run_they_break() {
        let evidence = derived(
            &[reading(40.0, T0), reading(30.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::PercentageDecrease), 1);

        let evidence = derived(
            &[reading(99.0, T0), reading(100.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::PercentageSaturation), 1);
    }

    #[test]
    fn a_later_clean_reading_starts_a_new_run_after_a_break() {
        // 40 → 30 breaks the run; 30 → 45 is a fresh, perfectly good interval.
        let evidence = derived(
            &[reading(40.0, T0), reading(30.0, T0 + 600), reading(45.0, T0 + 1_200)],
            &[usage(T0 + 700, 1_500)],
        );
        assert_eq!(
            only_intervals(&evidence),
            vec![Interval {
                from_pct: 30,
                to_pct: 45,
                tokens: 1_500,
                t0: T0 + 600,
                t1: T0 + 1_200,
            }],
        );
    }

    #[test]
    fn coverage_must_reach_back_past_the_earlier_anchor() {
        let mut to = reading(50.0, T0 + 600);
        // Capture is proven only from inside the stretch, so what happened
        // before it is unknown rather than nothing.
        to.provenance.covered_from = Some(T0 + 300);
        let evidence = derived(&[reading(40.0, T0), to.clone()], &[usage(T0 + 400, 1_000)]);
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::UnprovenSourceCompleteness), 1);

        // Correcting the same fact — the next evaluation, not a rewrite — is all
        // it takes for the interval to come back.
        let mut corrected = to;
        corrected.provenance.covered_from = Some(T0 - 1);
        let evidence = derived(&[reading(40.0, T0), corrected], &[usage(T0 + 400, 1_000)]);
        assert_eq!(only_intervals(&evidence).len(), 1);
    }

    #[test]
    fn a_known_external_activity_fact_withdraws_the_interval_it_overlaps() {
        let mut to = reading(50.0, T0 + 600);
        to.provenance.external_activity = Some("codex-web".to_string());
        let evidence = derived(&[reading(40.0, T0), to], &[usage(T0 + 100, 1_000)]);
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::KnownExternalActivity), 1);
    }

    #[test]
    fn readings_of_one_instant_bound_nothing() {
        let evidence = derived(
            &[reading(40.0, T0), reading(50.0, T0)],
            &[usage(T0, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::AmbiguousReadingOrder), 1);
    }

    #[test]
    fn movement_with_no_local_usage_is_detected_elsewhere_activity() {
        let evidence = derived(&[reading(40.0, T0), reading(50.0, T0 + 600)], &[]);
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::ZeroLocalUsage), 1);
    }

    #[test]
    fn a_later_record_changes_the_answer_with_nothing_to_invalidate() {
        // The same Readings, read twice against different Ledgers: derivation
        // owns no stored total, so a late or superseding Record simply counts.
        let readings = [reading(40.0, T0), reading(50.0, T0 + 600)];
        let before = derived(&readings, &[usage(T0 + 100, 1_000)]);
        let after = derived(&readings, &[usage(T0 + 100, 1_000), usage(T0 + 200, 250)]);
        assert_eq!(only_intervals(&before)[0].tokens, 1_000);
        assert_eq!(only_intervals(&after)[0].tokens, 1_250);
    }

    #[test]
    fn a_decrease_inside_the_jitter_band_rejects_the_grouping() {
        // 90 → 5 → 40 with stamps 117s and 120s apart. Banding on the stamps
        // alone would call all three one epoch and hand the estimator a 5 → 40
        // movement belonging to the window before the reset. A fall inside a
        // band is the signature of a reset the jitter hid, and an ambiguous
        // grouping is rejected rather than guessed at.
        let mut before = reading(90.0, T0);
        before.resets_at = RESET;
        let mut after = reading(5.0, T0 + 100);
        after.resets_at = RESET + 117;
        let mut later = reading(40.0, T0 + 200);
        later.resets_at = RESET + 120;

        let evidence = derived(&[before, after, later], &[usage(T0 + 150, 5_000)]);
        // 5 → 40 is real evidence — for the window that started at the reset,
        // which is where it must be filed. The one thing it may never be is the
        // 90% window's, whose own movement ended when the window did.
        assert_eq!(evidence.partitions.len(), 1);
        assert_eq!(evidence.partitions[0].epoch, RESET + 117);
        assert_eq!(
            evidence.partitions[0].intervals,
            vec![Interval { from_pct: 5, to_pct: 40, tokens: 5_000, t0: T0 + 100, t1: T0 + 200 }],
        );
        // And the fall itself is a reset, not a decrease inside one window.
        assert_eq!(refused(&evidence, ReasonCode::ResetBoundary), 1);
        assert_eq!(refused(&evidence, ReasonCode::PercentageDecrease), 0);
    }

    #[test]
    fn a_reading_of_another_partition_ends_the_run_it_interrupts() {
        // The middle Reading says the account changed and changed back. The
        // tokens either side of it are not one stretch, and a run cannot step
        // over the Reading that says so.
        let mut theirs = reading(40.0, T0 + 300);
        theirs.provenance.account_id = Some("acct-b".to_string());
        let evidence = derived(
            &[reading(40.0, T0), theirs, reading(50.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::IdentityChange), 2);
    }

    #[test]
    fn a_reading_that_proves_nothing_ends_the_run_it_interrupts() {
        let mut unprovable = reading(40.0, T0 + 300);
        unprovable.provenance.account_id = None;
        let evidence = derived(
            &[reading(40.0, T0), unprovable, reading(50.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::MissingAccountIdentity), 1);
    }

    #[test]
    fn a_movement_lost_to_a_reset_always_says_so() {
        // Stamps that drift past the band one step at a time: whichever way the
        // epochs fall, no movement may vanish without a reason beside it.
        let stamps = [RESET, RESET + 500, RESET + 1_000];
        let mut readings = Vec::new();
        for (i, stamp) in stamps.iter().enumerate() {
            let mut r = reading(10.0 * (i as f64 + 1.0), T0 + 100 * i as i64);
            r.resets_at = *stamp;
            readings.push(r);
        }
        let evidence = derived(&readings, &[usage(T0 + 50, 1_000), usage(T0 + 150, 1_000)]);
        let movements =
            only_intervals(&evidence).len() + evidence.rejections.values().sum::<usize>();
        assert_eq!(movements, 2, "every pair is either an interval or a reason: {evidence:?}");
    }

    #[test]
    fn any_identity_fact_changing_separates_evidence() {
        type Change = fn(&mut LimitReading);
        let changes: [Change; 5] = [
            |r| r.provenance.account_id = Some("acct-b".to_string()),
            |r| r.plan = Some("pro".to_string()),
            |r| r.provenance.metering_regime = Some("codex:rate_limits+x".to_string()),
            |r| r.provenance.limit_id = Some("codex:w300".to_string()),
            |r| r.provenance.model_scope = Some(ModelScope::Models(vec!["gpt-5.4".to_string()])),
        ];
        for change in changes {
            let mut moved = reading(50.0, T0 + 600);
            change(&mut moved);
            let evidence = derived(&[reading(40.0, T0), moved], &[usage(T0 + 100, 1_000)]);
            assert!(only_intervals(&evidence).is_empty());
            assert_eq!(refused(&evidence, ReasonCode::IdentityChange), 1);
        }
    }

    #[test]
    fn the_read_pairs_stored_readings_with_the_ledger_they_belong_to() {
        use crate::db;
        use crate::types::{CtxTokens, UsageEvent};

        let dir = tempfile::tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_limit_readings(&mut conn, &[reading(40.0, T0), reading(50.0, T0 + 600)])
            .unwrap();

        let event = |dedup_key: &str, timestamp: i64, account: Option<&str>, input: i64| {
            let mut event = UsageEvent {
                dedup_key: dedup_key.to_string(),
                source: "codex".to_string(),
                timestamp,
                model: Some("gpt-5.4-codex".to_string()),
                project: None,
                api_calls: 1,
                input_tokens: input,
                output_tokens: 20,
                cache_read_tokens: 5,
                cache_write_5m_tokens: 3,
                cache_write_1h_tokens: 2,
                source_file: "rollout.jsonl".to_string(),
                session_id: None,
                reasoning_tokens: Some(9),
                ctx: CtxTokens::default(),
            };
            if account.is_none() {
                event.dedup_key.push_str(":anon");
            }
            event
        };
        db::insert_events(
            &mut conn,
            &[
                event("codex:in", T0 + 100, Some("acct-a"), 100),
                event("codex:before", T0 - 10, Some("acct-a"), 100),
                event("codex:anon", T0 + 200, None, 100),
            ],
        )
        .unwrap();
        // Only the account-bearing Record can participate, so only it is read.
        conn.execute(
            "UPDATE events SET account_id = 'acct-a' WHERE dedup_key IN ('codex:in', 'codex:before')",
            [],
        )
        .unwrap();

        let evidence = evidence(&conn, T0 - 3_600).unwrap().unwrap();
        // Canonical tokens: 100 + 20 + 5 + 3 + 2, with reasoning left out
        // because it classifies tokens already counted.
        assert_eq!(
            only_intervals(&evidence),
            vec![Interval { from_pct: 40, to_pct: 50, tokens: 130, t0: T0, t1: T0 + 600 }],
        );
    }

    #[test]
    fn a_non_finite_percentage_is_a_technical_failure_not_a_readiness_state() {
        // The specification calls this an invariant failure: it must reject the
        // command through the error path rather than looking like evidence
        // nobody has yet.
        let failed = derive(
            &[reading(f64::NAN, T0), reading(50.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert_eq!(
            failed,
            Err(NonFinitePercentage { source: "codex".to_string(), observed_at: T0 }),
        );
    }

    #[test]
    fn a_break_between_consecutive_readings_says_which_kind_it_was() {
        // The card shows these one after another; evidence cannot join them, and
        // which reason it was is the difference between Gathering and Blocked.
        let mut other_account = reading(50.0, T0 + 600);
        other_account.provenance.account_id = Some("acct-b".to_string());
        let changed = derived(&[reading(40.0, T0), other_account], &[usage(T0 + 100, 1_000)]);
        assert_eq!(refused(&changed, ReasonCode::IdentityChange), 1);
        assert_eq!(refused(&changed, ReasonCode::ResetBoundary), 0);

        let mut next_epoch = reading(5.0, T0 + 600);
        next_epoch.resets_at = RESET + 604_800;
        let reset = derived(&[reading(40.0, T0), next_epoch], &[usage(T0 + 100, 1_000)]);
        assert_eq!(refused(&reset, ReasonCode::ResetBoundary), 1);
        assert_eq!(refused(&reset, ReasonCode::IdentityChange), 0);
    }

    #[test]
    fn a_fact_on_a_reading_the_walk_passed_over_still_spoils_the_movement() {
        // 40 → 40 → 42 is one interval, and the middle Reading is inside it. A
        // known external-activity fact there has to reject the movement it sits
        // in, not be skipped along with the Reading that carried it.
        let mut middle = reading(40.0, T0 + 300);
        middle.provenance.external_activity = Some("codex-web".to_string());
        let evidence = derived(
            &[reading(40.0, T0), middle, reading(42.0, T0 + 600)],
            &[usage(T0 + 100, 1_000)],
        );
        assert!(only_intervals(&evidence).is_empty());
        assert_eq!(refused(&evidence, ReasonCode::KnownExternalActivity), 1);
    }

    #[test]
    fn the_anchors_own_fact_belongs_to_the_stretch_that_ended_there() {
        // External activity recorded at the anchor describes what happened
        // before it, which is the previous interval's business — the movement
        // that starts there is not spoiled by it.
        let mut anchor = reading(40.0, T0);
        anchor.provenance.external_activity = Some("codex-web".to_string());
        let evidence = derived(&[anchor, reading(50.0, T0 + 600)], &[usage(T0 + 100, 1_000)]);
        assert_eq!(only_intervals(&evidence).len(), 1);
    }
}

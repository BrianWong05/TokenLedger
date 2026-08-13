//! Whether a Limit Token Estimate may be shown, and when time alone could
//! change that answer.
//!
//! Pure, like the estimator it reads: the evaluation instant is injected, so
//! Stale can be *reconstructed* from current evidence rather than remembered
//! from a run that happened to see Ready. Only Ready carries a number, there is
//! no grace period, and every state returns to Ready by itself when the evidence
//! does.


use std::collections::BTreeMap;

use crate::limits_estimator::{estimates, recency_horizon, Candidate, Estimate};
use crate::limits_evidence::{PartitionEvidence, ReasonCode, SeriesKey};
use crate::types::LimitReading;

/// The estimator policy these evaluations were produced under. A change to any
/// rule changes this, so a cached answer can never outlive the policy that made
/// it.
pub const POLICY_VERSION: &str = "limit-token-estimate-v1";

/// How many recent completed epochs a core needs.
const REQUIRED_EPOCHS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessState {
    /// The current identity or Source completeness is unproven, so there is
    /// nothing to be sure about yet.
    Blocked,
    /// Enough recent evidence, and it agrees.
    Ready,
    /// Enough recent evidence, and it does not agree.
    Unstable,
    /// Not enough recent evidence, but the same policy over older evidence
    /// found agreement that has since aged out.
    Stale,
    /// Not enough recent evidence, and none older that ever agreed.
    Gathering,
}

/// What was weighed, in codes and counts — never in prose, which is the
/// frontend's to write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Explanation {
    pub reason_codes: Vec<ReasonCode>,
    pub rejections: BTreeMap<ReasonCode, usize>,
    pub qualifying_epochs: usize,
    pub required_epochs: usize,
    pub recent_cutoff_at: i64,
    pub newest_completed_epoch_at: Option<i64>,
    /// At most five, the ones the policy inspected.
    pub candidates: Vec<Candidate>,
    /// The narrowest and widest ratio among them.
    pub ratio_range: Option<(f64, f64)>,
    /// Where every candidate's endpoint rounding could agree, if anywhere.
    pub quantization_intersection: Option<(f64, Option<f64>)>,
}

/// One Limit's estimate, and when time alone could change it.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub state: ReadinessState,
    /// Only Ready carries a number.
    pub tokens_per_pct: Option<f64>,
    pub evaluated_at: i64,
    /// The earliest future second at which time by itself changes this answer,
    /// or `None` when nothing is waiting on the clock.
    pub next_evaluation_at: Option<i64>,
    pub policy_version: &'static str,
    pub explanation: Explanation,
}

/// Why the current Reading cannot anchor an estimate, if it cannot.
///
/// An expired Reading is not a current one: the window it described has already
/// reset, so it proves no current Partition even while the card goes on drawing
/// its fallback.
fn blocked_because(current: Option<&LimitReading>, evaluated_at: i64) -> Option<ReasonCode> {
    let Some(current) = current else {
        return Some(ReasonCode::NoCurrentReading);
    };
    if current.resets_at <= evaluated_at {
        return Some(ReasonCode::NoCurrentReading);
    }
    if let Err(missing) = SeriesKey::of(current) {
        return Some(missing);
    }
    current
        .provenance
        .covered_from
        .is_none()
        .then_some(ReasonCode::UnprovenSourceCompleteness)
}

/// Evaluate one Limit: its current Reading, the evidence of every Series it has
/// ever had, and the single evaluation instant.
///
/// `partitions` may hold Series this Limit has moved on from — an older account,
/// a changed plan. They are not consulted: a change of identity never carries an
/// estimate forward, and the Series in force starts from its own evidence.
pub fn evaluate(
    current: Option<&LimitReading>,
    partitions: &[PartitionEvidence],
    evaluated_at: i64,
) -> Evaluation {
    let mut explanation = Explanation {
        required_epochs: REQUIRED_EPOCHS,
        ..Explanation::default()
    };

    if let Some(blocked) = blocked_because(current, evaluated_at) {
        explanation.reason_codes.push(blocked);
        explanation.recent_cutoff_at = evaluated_at - recency_horizon(None);
        return Evaluation {
            state: ReadinessState::Blocked,
            tokens_per_pct: None,
            evaluated_at,
            next_evaluation_at: next_evaluation_at(current, &[], evaluated_at),
            policy_version: POLICY_VERSION,
            explanation,
        };
    }

    // Blocked has been ruled out, so the current Reading proves its Series.
    let current = current.expect("a Blocked check without a Reading returns above");
    let series = SeriesKey::of(current).expect("its Series was proven above");
    let own: Vec<PartitionEvidence> =
        partitions.iter().filter(|p| p.series == series).cloned().collect();

    let estimate = estimate_of(&own, evaluated_at);
    let horizon = recency_horizon(own.iter().find_map(|p| p.window_minutes));

    explanation.rejections = estimate.rejections.clone();
    explanation.qualifying_epochs = estimate.candidates.len();
    explanation.recent_cutoff_at = evaluated_at - horizon;
    explanation.newest_completed_epoch_at =
        own.iter().map(|p| p.epoch).filter(|e| *e <= evaluated_at).max();
    explanation.ratio_range = ratio_range(&estimate.candidates);
    explanation.quantization_intersection = intersection(&estimate.candidates);
    explanation.candidates = estimate.candidates.clone();

    let state = if estimate.tokens_per_pct.is_some() {
        ReadinessState::Ready
    } else if estimate.candidates.len() >= REQUIRED_EPOCHS {
        ReadinessState::Unstable
    } else if aged_out_core(&own, evaluated_at) {
        explanation.reason_codes.push(ReasonCode::HistoricalCoreAgedOut);
        ReadinessState::Stale
    } else {
        ReadinessState::Gathering
    };

    explanation
        .reason_codes
        .extend(estimate.rejections.keys().copied());

    Evaluation {
        state,
        // Only Ready carries a number, whatever the estimator found.
        tokens_per_pct: (state == ReadinessState::Ready)
            .then_some(estimate.tokens_per_pct)
            .flatten(),
        evaluated_at,
        next_evaluation_at: next_evaluation_at(Some(current), &estimate.candidates, evaluated_at),
        policy_version: POLICY_VERSION,
        explanation,
    }
}

/// The estimator's answer for one Series' evidence at one instant.
fn estimate_of(own: &[PartitionEvidence], evaluated_at: i64) -> Estimate {
    estimates(own, evaluated_at).into_iter().next().map(|(_, e)| e).unwrap_or_default()
}

/// Whether this evidence ever agreed under today's policy.
///
/// Reconstructed, never remembered: the same rules are replayed at each
/// completed epoch's own clock, newest first, and the walk stops at the first
/// instant that would have been Ready. That an earlier process happened to see
/// Ready is not evidence of anything, and is not consulted.
fn aged_out_core(own: &[PartitionEvidence], evaluated_at: i64) -> bool {
    let mut clocks: Vec<i64> =
        own.iter().map(|p| p.epoch).filter(|e| *e <= evaluated_at).collect();
    clocks.sort_unstable_by(|a, b| b.cmp(a));
    clocks.dedup();
    clocks
        .into_iter()
        .any(|clock| estimate_of(own, clock).tokens_per_pct.is_some())
}

fn ratio_range(candidates: &[Candidate]) -> Option<(f64, f64)> {
    let min = candidates.iter().map(|c| c.ratio).reduce(f64::min)?;
    let max = candidates.iter().map(|c| c.ratio).reduce(f64::max)?;
    Some((min, max))
}

/// Where every candidate's rounding could agree — the diagnostic the explanation
/// reports, not a confidence in any form.
fn intersection(candidates: &[Candidate]) -> Option<(f64, Option<f64>)> {
    let lower = candidates.iter().map(|c| c.quantization.lower).reduce(f64::max)?;
    let upper = candidates.iter().filter_map(|c| c.quantization.upper).reduce(f64::min);
    match upper {
        Some(upper) if lower > upper => None,
        upper => Some((lower, upper)),
    }
}

/// The earliest future second at which the clock alone changes this answer: the
/// active window's reset, or the moment a candidate stops being recent.
///
/// Recency is inclusive, so a candidate whose epoch ended at `e` under a horizon
/// `h` is still recent at `e + h` and expires the second after.
fn next_evaluation_at(
    current: Option<&LimitReading>,
    candidates: &[Candidate],
    evaluated_at: i64,
) -> Option<i64> {
    let horizon = recency_horizon(current.and_then(|r| r.window_minutes));
    current
        .map(|r| r.resets_at)
        .into_iter()
        .chain(candidates.iter().map(|c| c.ended_at() + horizon + 1))
        .filter(|at| *at > evaluated_at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits_estimator::Candidate;
    use crate::limits_evidence::{Interval, PartitionEvidence, SeriesKey};
    use crate::types::{LimitReading, ModelScope, ReadingProvenance};

    const NOW: i64 = 1_786_752_000;
    const DAY: i64 = 86_400;

    fn series() -> SeriesKey {
        SeriesKey {
            source: "codex".to_string(),
            account_id: "acct-a".to_string(),
            plan: "plus".to_string(),
            metering_regime: "codex:rate_limits".to_string(),
            limit_id: "codex:w10080".to_string(),
            model_scope: "all".to_string(),
        }
    }

    /// The Reading the card is showing: proven, and its window still running.
    fn current() -> LimitReading {
        LimitReading {
            source: "codex".to_string(),
            window_key: "w10080".to_string(),
            window_minutes: Some(10_080),
            used_pct: 40.0,
            resets_at: NOW + DAY,
            observed_at: NOW - 60,
            via: "logs".to_string(),
            plan: Some("plus".to_string()),
            provenance: ReadingProvenance {
                account_id: Some("acct-a".to_string()),
                metering_regime: Some("codex:rate_limits".to_string()),
                limit_id: Some("codex:w10080".to_string()),
                model_scope: Some(ModelScope::All),
                source_order: Some(1),
                covered_from: Some(NOW - 90 * DAY),
                external_activity: None,
            },
        }
    }

    /// A completed epoch carrying one qualifying run of `movement` points.
    fn epoch(ended_days_ago: i64, movement: i64, tokens: i64) -> PartitionEvidence {
        let ended = NOW - ended_days_ago * DAY;
        let half = movement / 2;
        let (first, second) = (tokens / 2, tokens - tokens / 2);
        PartitionEvidence {
            series: series(),
            epoch: ended,
            window_minutes: Some(10_080),
            intervals: vec![
                Interval { from_pct: 0, to_pct: half, tokens: first, t0: ended - 7_200, t1: ended - 3_600 },
                Interval {
                    from_pct: half,
                    to_pct: movement,
                    tokens: second,
                    t0: ended - 3_600,
                    t1: ended - 1_800,
                },
            ],
        }
    }

    /// Three epochs that agree — the shape of a Ready estimate.
    fn agreeing(days: [i64; 3]) -> Vec<PartitionEvidence> {
        vec![
            epoch(days[0], 20, 2_000),
            epoch(days[1], 20, 2_020),
            epoch(days[2], 20, 1_980),
        ]
    }

    fn code(evaluation: &Evaluation, reason: ReasonCode) -> bool {
        evaluation.explanation.reason_codes.contains(&reason)
    }

    #[test]
    fn a_limit_with_no_current_reading_is_blocked() {
        let evaluation = evaluate(None, &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Blocked);
        assert_eq!(evaluation.tokens_per_pct, None);
        assert!(code(&evaluation, ReasonCode::NoCurrentReading));
    }

    #[test]
    fn an_expired_reading_proves_no_current_partition() {
        // The window it described has already reset, so what it says is history.
        // The card may still draw its fallback; the estimate may not.
        let mut expired = current();
        expired.resets_at = NOW - 60;
        let evaluation = evaluate(Some(&expired), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Blocked);
        assert!(code(&evaluation, ReasonCode::NoCurrentReading));
    }

    #[test]
    fn blocked_outranks_every_other_state() {
        // The evidence would be Ready twice over; the current Reading cannot
        // prove which account it belongs to, so nothing may be shown.
        let mut unproven = current();
        unproven.provenance.account_id = None;
        let evaluation = evaluate(Some(&unproven), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Blocked);
        assert!(code(&evaluation, ReasonCode::MissingAccountIdentity));
        assert_eq!(evaluation.tokens_per_pct, None);
    }

    #[test]
    fn unproven_current_completeness_is_blocked_too() {
        let mut uncovered = current();
        uncovered.provenance.covered_from = None;
        let evaluation = evaluate(Some(&uncovered), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Blocked);
        assert!(code(&evaluation, ReasonCode::UnprovenSourceCompleteness));
    }

    #[test]
    fn three_agreeing_recent_epochs_are_ready() {
        let evaluation = evaluate(Some(&current()), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Ready);
        assert_eq!(evaluation.tokens_per_pct, Some(100.0));
        assert_eq!(evaluation.explanation.qualifying_epochs, 3);
    }

    #[test]
    fn evidence_that_contradicts_itself_is_unstable_at_once() {
        // Three epochs, no two of which share a rounding: enough evidence, and
        // no agreement. There is no grace period and no former answer to keep.
        let disagreeing =
            vec![epoch(1, 20, 2_000), epoch(2, 20, 6_000), epoch(3, 20, 12_000)];
        let evaluation = evaluate(Some(&current()), &disagreeing, NOW);
        assert_eq!(evaluation.state, ReadinessState::Unstable);
        assert_eq!(evaluation.tokens_per_pct, None);
    }

    #[test]
    fn a_ready_estimate_is_withdrawn_and_restored_by_the_evidence_alone() {
        let ready = agreeing([1, 2, 3]);
        assert_eq!(evaluate(Some(&current()), &ready, NOW).state, ReadinessState::Ready);

        // One epoch that agrees with nothing does not withdraw anything: at four
        // candidates a core may leave one out, which is what that rule is for.
        let mut one_outlier = ready.clone();
        one_outlier.push(epoch(4, 20, 40_000));
        let tolerated = evaluate(Some(&current()), &one_outlier, NOW);
        assert_eq!(tolerated.state, ReadinessState::Ready);
        assert_eq!(tolerated.tokens_per_pct, Some(100.0));

        // A second one does. At five candidates a core needs four, and no four
        // of these agree — so the estimate is withdrawn the moment the evidence
        // stops supporting it, with no grace period and no former answer kept.
        let mut outweighed = one_outlier;
        outweighed.push(epoch(5, 20, 41_000));
        let withdrawn = evaluate(Some(&current()), &outweighed, NOW);
        assert_eq!(withdrawn.state, ReadinessState::Unstable);
        assert_eq!(withdrawn.tokens_per_pct, None);

        // And restored by nothing more than the evidence agreeing again.
        assert_eq!(evaluate(Some(&current()), &ready, NOW).state, ReadinessState::Ready);
    }

    #[test]
    fn too_few_recent_epochs_and_no_history_is_gathering() {
        let evaluation = evaluate(Some(&current()), &[epoch(1, 20, 2_000)], NOW);
        assert_eq!(evaluation.state, ReadinessState::Gathering);
        assert_eq!(evaluation.explanation.qualifying_epochs, 1);
        assert!(code(&evaluation, ReasonCode::InsufficientRecentEpochs));
    }

    #[test]
    fn a_core_that_has_aged_out_is_stale_rather_than_gathering() {
        // Three agreeing epochs, all older than the six-window horizon: too few
        // recent candidates now, but replaying the policy at the newest of them
        // finds the Ready this Series used to have.
        let aged = agreeing([50, 51, 52]);
        let evaluation = evaluate(Some(&current()), &aged, NOW);
        assert_eq!(evaluation.state, ReadinessState::Stale);
        assert_eq!(evaluation.tokens_per_pct, None, "only Ready carries a number");
        assert!(code(&evaluation, ReasonCode::HistoricalCoreAgedOut));
    }

    #[test]
    fn stale_is_reconstructed_not_remembered() {
        // The same evidence at an instant when it was still recent is Ready.
        // Nothing was stored between the two calls: the difference is the clock.
        let aged = agreeing([50, 51, 52]);
        let then = evaluate(Some(&current()), &aged, NOW - 50 * DAY);
        assert_eq!(then.state, ReadinessState::Ready);
        assert_eq!(evaluate(Some(&current()), &aged, NOW).state, ReadinessState::Stale);
    }

    #[test]
    fn history_that_never_agreed_is_gathering_not_stale() {
        let never = vec![epoch(50, 20, 2_000), epoch(51, 20, 6_000), epoch(52, 20, 12_000)];
        assert_eq!(
            evaluate(Some(&current()), &never, NOW).state,
            ReadinessState::Gathering,
        );
    }

    #[test]
    fn the_next_evaluation_is_the_soonest_thing_time_alone_can_change() {
        // The active window resets in a day; the oldest candidate ages out of a
        // 42-day horizon later than that.
        let evaluation = evaluate(Some(&current()), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.next_evaluation_at, Some(NOW + DAY));

        // Move the reset far out, and the earliest expiry is what remains: an
        // epoch that ended `e` ago expires at `e + h + 1`, the inclusive rule's
        // first second of no longer counting.
        let mut later = current();
        later.resets_at = NOW + 40 * DAY;
        let evaluation = evaluate(Some(&later), &agreeing([1, 2, 3]), NOW);
        let horizon = 6 * 10_080 * 60;
        assert_eq!(evaluation.next_evaluation_at, Some(NOW - 3 * DAY + horizon + 1));
    }

    #[test]
    fn nothing_in_the_future_means_no_next_evaluation() {
        // No current Reading to reset, and no candidates to age out.
        let evaluation = evaluate(None, &[], NOW);
        assert_eq!(evaluation.next_evaluation_at, None);
    }

    #[test]
    fn the_explanation_says_what_was_weighed() {
        let evaluation = evaluate(Some(&current()), &agreeing([1, 2, 3]), NOW);
        let explanation = &evaluation.explanation;
        assert_eq!(explanation.required_epochs, 3);
        assert_eq!(explanation.candidates.len(), 3);
        assert_eq!(explanation.newest_completed_epoch_at, Some(NOW - DAY));
        assert_eq!(explanation.recent_cutoff_at, NOW - 6 * 10_080 * 60);
        let (min, max) = explanation.ratio_range.expect("Ready has candidates");
        assert_eq!((min, max), (99.0, 101.0));
        assert!(explanation.quantization_intersection.is_some());
    }

    #[test]
    fn a_series_the_current_reading_left_behind_carries_nothing_forward() {
        // The epochs belong to the account that was signed in before; the
        // current Reading names another. Its evidence is its own, and there is
        // none of it yet.
        let mut moved_on = current();
        moved_on.provenance.account_id = Some("acct-b".to_string());
        let evaluation = evaluate(Some(&moved_on), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Gathering);
        assert_eq!(evaluation.explanation.candidates.len(), 0);
    }

    #[test]
    fn every_evaluation_names_the_policy_that_produced_it() {
        assert_eq!(evaluate(None, &[], NOW).policy_version, POLICY_VERSION);
        assert_eq!(POLICY_VERSION, "limit-token-estimate-v1");
    }

    /// The candidates a Ready evaluation reports are the estimator's own.
    #[test]
    fn ready_reports_the_candidates_its_core_was_drawn_from() {
        let evaluation = evaluate(Some(&current()), &agreeing([1, 2, 3]), NOW);
        let epochs: Vec<i64> =
            evaluation.explanation.candidates.iter().map(Candidate::ended_at).collect();
        assert_eq!(epochs, vec![NOW - DAY, NOW - 2 * DAY, NOW - 3 * DAY]);
    }
}

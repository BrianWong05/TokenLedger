//! Whether a Limit Token Estimate may be shown, and when time alone could
//! change that answer.
//!
//! Pure, like the estimator it reads: the evaluation instant is injected, so
//! Stale can be *reconstructed* from current evidence rather than remembered
//! from a run that happened to see Ready. Only Ready carries a number, there is
//! no grace period, and every state returns to Ready by itself when the evidence
//! does.


use std::collections::BTreeMap;

use serde::Serialize;
use ts_rs::TS;

use crate::limits_estimator::{
    estimates, quantization_intersection, ratio_range, recency_horizon, Candidate, Estimate,
    Quantization,
};
use crate::limits_evidence::{PartitionEvidence, ReasonCode, SeriesKey};
use crate::types::LimitReading;

/// The policy these evaluations were produced under — every rule of it, from
/// what counts as evidence through the estimator's arithmetic to the states
/// named here. A change to any of them changes this, so a cached answer can
/// never outlive the policy that made it.
pub const POLICY_VERSION: &str = "limit-token-estimate-v1";

/// How many recent completed epochs a core needs.
const REQUIRED_EPOCHS: usize = 3;

/// On the wire this is `LimitEstimateState`: the contract's name for the same
/// five states, so the domain enum travels rather than being copied into a
/// parallel one — the treatment `ReasonCode` already gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../src/bindings/", rename = "LimitEstimateState")]
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
    /// Which of them the core was drawn from, by index into `candidates`. Empty
    /// unless Ready — the count the row reports is core membership, not every
    /// candidate epoch.
    pub core: Vec<usize>,
    /// The narrowest and widest ratio in the set the answer came from: the core
    /// where there is one, every candidate otherwise. Reporting all of them
    /// under Ready would contradict the state, since Ready asserts precisely
    /// that its own members agree.
    pub ratio_range: Option<(f64, f64)>,
    /// Where that same set's endpoint rounding agrees, if anywhere.
    pub quantization_intersection: Option<Quantization>,
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

/// The current Reading and the Series it proves, or why it can anchor nothing.
///
/// An expired Reading is not a current one: the window it described has already
/// reset, so it proves no current Partition even while the card goes on drawing
/// its fallback.
///
/// Completeness here is the presence of a coverage fact, not a reach: what the
/// row would show is derived from completed epochs, and every interval inside
/// them proved its own coverage back past its own anchor. This fact answers a
/// different question — whether local capture of this Source and account is
/// still proven at all — and a Source that cannot say is Blocked rather than
/// merely short of evidence.
fn anchor(
    current: Option<&LimitReading>,
    evaluated_at: i64,
) -> Result<(&LimitReading, SeriesKey), ReasonCode> {
    let current = current.ok_or(ReasonCode::NoCurrentReading)?;
    if current.resets_at <= evaluated_at {
        return Err(ReasonCode::NoCurrentReading);
    }
    let series = SeriesKey::of(current)?;
    if current.provenance.covered_from.is_none() {
        return Err(ReasonCode::UnprovenSourceCompleteness);
    }
    Ok((current, series))
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
    let (current, series) = match anchor(current, evaluated_at) {
        Ok(anchored) => anchored,
        Err(blocked) => {
            // Blocked still describes the evidence it has, rather than inventing
            // a horizon it never used: what was weighed is a fact about the
            // Ledger, not about the state.
            let horizon = recency_horizon(partitions.iter().find_map(|p| p.window_minutes));
            return Evaluation {
                state: ReadinessState::Blocked,
                tokens_per_pct: None,
                evaluated_at,
                next_evaluation_at: next_evaluation_at(None, &[], horizon, evaluated_at),
                policy_version: POLICY_VERSION,
                explanation: Explanation {
                    reason_codes: vec![blocked],
                    required_epochs: REQUIRED_EPOCHS,
                    recent_cutoff_at: evaluated_at - horizon,
                    newest_completed_epoch_at: newest_completed(partitions, evaluated_at),
                    ..Explanation::default()
                },
            };
        }
    };

    let own: Vec<PartitionEvidence> =
        partitions.iter().filter(|p| p.series == series).cloned().collect();
    let estimate = estimate_of(&own, evaluated_at);

    let state = if estimate.tokens_per_pct.is_some() {
        ReadinessState::Ready
    } else if estimate.candidates.len() >= REQUIRED_EPOCHS {
        ReadinessState::Unstable
    } else if aged_out_core(&own, evaluated_at) {
        ReadinessState::Stale
    } else {
        ReadinessState::Gathering
    };

    // The set the answer came from: the core where there is one, and every
    // candidate where there is not.
    let described: Vec<&Candidate> = if estimate.core.is_empty() {
        estimate.candidates.iter().collect()
    } else {
        estimate.core.iter().map(|&i| &estimate.candidates[i]).collect()
    };

    Evaluation {
        state,
        // Only Ready carries a number, whatever the estimator found.
        tokens_per_pct: (state == ReadinessState::Ready)
            .then_some(estimate.tokens_per_pct)
            .flatten(),
        evaluated_at,
        next_evaluation_at: next_evaluation_at(
            Some(current),
            &estimate.candidates,
            estimate.horizon,
            evaluated_at,
        ),
        policy_version: POLICY_VERSION,
        explanation: Explanation {
            reason_codes: evaluation_codes(&estimate, state),
            qualifying_epochs: estimate.candidates.len(),
            required_epochs: REQUIRED_EPOCHS,
            recent_cutoff_at: evaluated_at - estimate.horizon,
            newest_completed_epoch_at: newest_completed(&own, evaluated_at),
            ratio_range: ratio_range(&described),
            quantization_intersection: quantization_intersection(&described),
            core: estimate.core,
            candidates: estimate.candidates,
            // Counted by reason rather than listed: the codes and their tallies
            // are the same facts, and the diagnostic path is what expands them.
            rejections: estimate.rejections,
        },
    }
}

/// The codes that speak for the evaluation as a whole, as against the ones
/// counted per epoch.
///
/// The specification draws that line itself: `rejections` *"aggregates every
/// interval, run, or epoch rejection by reason"*, while the codes beside it are
/// facts about the answer. So an epoch with no qualifying run stays a tally,
/// and "there were not enough recent epochs" — or that their ranges never met —
/// is what the explanation says out loud.
fn evaluation_codes(estimate: &Estimate, state: ReadinessState) -> Vec<ReasonCode> {
    let mut codes: Vec<ReasonCode> = estimate
        .rejections
        .keys()
        .copied()
        .filter(|code| {
            matches!(
                code,
                ReasonCode::InsufficientRecentEpochs
                    | ReasonCode::QuantizationRangesDisjoint
                    | ReasonCode::RatioSpreadExceeded
                    | ReasonCode::CompetingStableCores
            )
        })
        .collect();
    if state == ReadinessState::Stale {
        codes.push(ReasonCode::HistoricalCoreAgedOut);
    }
    codes
}

/// The newest epoch that has ended by the evaluation instant.
fn newest_completed(partitions: &[PartitionEvidence], evaluated_at: i64) -> Option<i64> {
    partitions.iter().map(|p| p.epoch).filter(|e| *e <= evaluated_at).max()
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
/// ponytail: one full replay per completed epoch, so quadratic in a Series'
/// epochs. It is bounded twice over — by the horizon the caller read under, and
/// by stopping at the first proof — and it runs only on the path that has too
/// few recent candidates to be Ready. Page the replay if a Series ever holds
/// enough epochs for that to show.
fn aged_out_core(own: &[PartitionEvidence], evaluated_at: i64) -> bool {
    let mut clocks: Vec<i64> =
        own.iter().map(|p| p.epoch).filter(|e| *e <= evaluated_at).collect();
    clocks.sort_unstable_by(|a, b| b.cmp(a));
    clocks.dedup();
    clocks
        .into_iter()
        .any(|clock| estimate_of(own, clock).tokens_per_pct.is_some())
}

/// The earliest future second at which the clock alone changes this answer: the
/// active window's reset, or the moment a candidate stops being recent.
///
/// Recency is inclusive, so a candidate whose epoch ended at `e` under a horizon
/// `h` is still recent at `e + h` and expires the second after.
fn next_evaluation_at(
    current: Option<&LimitReading>,
    candidates: &[Candidate],
    horizon: i64,
    evaluated_at: i64,
) -> Option<i64> {
    current
        .map(|r| r.resets_at)
        .into_iter()
        .chain(candidates.iter().map(|c| c.epoch_ended_at + horizon + 1))
        .filter(|at| *at > evaluated_at)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// One movement only: enough span, not enough movements to qualify.
    fn chain_of(movement: i64, tokens: i64, t0: i64) -> Vec<Interval> {
        vec![Interval { from_pct: 0, to_pct: movement, tokens, t0, t1: t0 + 60 }]
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
    fn ready_describes_the_set_its_answer_came_from() {
        // Three agreeing epochs and one wild outlier. The core is the three, and
        // the ratios and the rounding the explanation reports must be theirs:
        // reporting all four would have Ready carrying a range that disagrees
        // with itself and an intersection of nothing, which is exactly what
        // Ready asserts cannot be true of its own members.
        let mut with_outlier = agreeing([1, 2, 3]);
        with_outlier.push(epoch(4, 20, 40_000));

        let evaluation = evaluate(Some(&current()), &with_outlier, NOW);
        assert_eq!(evaluation.state, ReadinessState::Ready);
        assert_eq!(evaluation.explanation.candidates.len(), 4, "all four were weighed");
        assert_eq!(evaluation.explanation.core.len(), 3, "three of them agreed");
        assert_eq!(evaluation.explanation.ratio_range, Some((99.0, 101.0)));
        assert!(evaluation.explanation.quantization_intersection.is_some());
    }

    #[test]
    fn one_horizon_measures_both_the_cutoff_and_the_expiry() {
        // The current Reading names no window; the evidence does. Taking the
        // horizon from the Reading would schedule an expiry seven days out for
        // candidates the cutoff keeps for forty-two, so the real expiry would
        // never be scheduled at all.
        let mut nameless = current();
        nameless.window_minutes = None;
        nameless.resets_at = NOW + 400 * DAY;

        let evaluation = evaluate(Some(&nameless), &agreeing([1, 2, 3]), NOW);
        let horizon = 6 * 10_080 * 60;
        assert_eq!(evaluation.explanation.recent_cutoff_at, NOW - horizon);
        assert_eq!(evaluation.next_evaluation_at, Some(NOW - 3 * DAY + horizon + 1));
    }

    #[test]
    fn blocked_describes_the_evidence_it_has_rather_than_inventing_any() {
        let evaluation = evaluate(None, &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Blocked);
        // The horizon is the one this Limit's own windows imply, not the
        // seven-day floor that would apply to a window naming no duration.
        assert_eq!(evaluation.explanation.recent_cutoff_at, NOW - 6 * 10_080 * 60);
        assert_eq!(evaluation.explanation.newest_completed_epoch_at, Some(NOW - DAY));
    }

    #[test]
    fn a_used_up_limit_still_carries_the_number_its_history_proved() {
        // The window is full. What filled it says nothing about what a point of
        // it is worth, which the completed epochs already settled — so Ready
        // stands. Hiding the selected figure on a used-up row is the card's.
        let mut full = current();
        full.used_pct = 100.0;
        let evaluation = evaluate(Some(&full), &agreeing([1, 2, 3]), NOW);
        assert_eq!(evaluation.state, ReadinessState::Ready);
        assert_eq!(evaluation.tokens_per_pct, Some(100.0));
    }

    #[test]
    fn an_epoch_that_offered_nothing_is_counted_not_announced() {
        // "No qualifying run" is a tally about one epoch; "not enough recent
        // epochs" is a fact about the answer. The explanation keeps them apart.
        let barren = PartitionEvidence {
            series: series(),
            epoch: NOW - DAY,
            window_minutes: Some(10_080),
            intervals: chain_of(12, 1_200, NOW - DAY - 3_600),
        };
        let evaluation = evaluate(Some(&current()), &[barren], NOW);
        assert_eq!(evaluation.state, ReadinessState::Gathering);
        assert!(code(&evaluation, ReasonCode::InsufficientRecentEpochs));
        assert!(!code(&evaluation, ReasonCode::NoQualifyingRun));
        assert_eq!(
            evaluation.explanation.rejections.get(&ReasonCode::NoQualifyingRun),
            Some(&1),
        );
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
            evaluation.explanation.candidates.iter().map(|c| c.epoch_ended_at).collect();
        assert_eq!(epochs, vec![NOW - DAY, NOW - 2 * DAY, NOW - 3 * DAY]);
    }
}

//! The estimator core: clean runs, one representative per completed epoch, a
//! unique stable core, and the median ratio it yields.
//!
//! Pure. No storage, no clock, no I/O — the evaluation instant is injected, so
//! the same evidence and the same instant always give the same answer. What it
//! deliberately does not do: decide a readiness state or name a next evaluation
//! time. Those read this and are the next ticket's.


use std::collections::BTreeMap;

use crate::limits_evidence::{Interval, PartitionEvidence, ReasonCode, SeriesKey};

/// What endpoint rounding alone could have hidden. Each displayed percentage
/// stands for anything within half a point, so a run's true movement is `d ± 1`
/// and its true ratio lies somewhere in here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantization {
    pub lower: f64,
    /// Unbounded at a single point of movement — `None` rather than an infinity
    /// no wire may carry.
    pub upper: Option<f64>,
}

/// A clean run: a maximal monotonic sequence of eligible intervals inside one
/// Partition. Its ratio is the whole run's, never a pooled sum and never an
/// average of its intervals.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// `T`, the canonical tokens across the run.
    pub tokens: i64,
    /// The run's first anchor, and the last Reading it reached.
    pub from: i64,
    pub through: i64,
    /// `d`, the displayed points it moved: last percentage less first. A run is
    /// built from positive movements only, so this is at least one.
    pub movement: i64,
    /// How many separate times it moved, which is not the same as how far.
    pub positive_movements: usize,
}

impl Run {
    pub fn ratio(&self) -> f64 {
        debug_assert!(self.movement > 0, "a run is built from positive movements");
        self.tokens as f64 / self.movement as f64
    }

    pub fn quantization(&self) -> Quantization {
        let tokens = self.tokens as f64;
        Quantization {
            lower: tokens / (self.movement + 1) as f64,
            upper: (self.movement > 1).then(|| tokens / (self.movement - 1) as f64),
        }
    }

    /// A run may represent its epoch only if it moved more than once and far
    /// enough that endpoint rounding cannot explain it.
    fn qualifies(&self) -> bool {
        self.positive_movements >= 2 && self.movement >= 10
    }
}

/// One completed epoch's representative — at most one per epoch, ever.
///
/// It carries the run's own bounds as well as its figures. The specification
/// asks that a run's raw Model composition and its contributing Reading and
/// Usage Record identities stay recoverable, while keeping them out of the
/// normal payload; `from`/`through` with the Series is what makes that
/// deterministic, since the contributors of a stretch are exactly the Records
/// its Source and account logged inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub epoch_ended_at: i64,
    pub tokens: i64,
    pub movement: i64,
    /// How many separate times it moved — the count the epoch summary reports,
    /// which is not the same as how far it moved.
    pub positive_movements: usize,
    /// The run's first anchor and last, exclusive and inclusive as membership is.
    pub from: i64,
    pub through: i64,
    pub ratio: f64,
    pub quantization: Quantization,
}

/// What the estimator makes of one Series.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Estimate {
    /// The recency horizon these candidates were judged under — six of the
    /// Limit's own windows, or seven days. Carried so that everything reporting
    /// or scheduling against it uses the one the answer was made with.
    pub horizon: i64,
    /// The median of the unique stable core's whole-run ratios, at full
    /// precision. `None` whenever no unique stable core exists — which is a
    /// state for the readiness machine to name, not this.
    pub tokens_per_pct: Option<f64>,
    /// The recent candidates inspected, newest first, never more than five.
    pub candidates: Vec<Candidate>,
    /// Which of them the core is made of, by index into `candidates`.
    pub core: Vec<usize>,
    pub rejections: BTreeMap<ReasonCode, usize>,
}

impl Estimate {
    fn refuse(&mut self, reason: ReasonCode) {
        *self.rejections.entry(reason).or_insert(0) += 1;
    }
}

/// Seven days, or six of the Limit's own windows where that is longer. A window
/// that names no duration gets the seven.
pub fn recency_horizon(window_minutes: Option<i64>) -> i64 {
    const SEVEN_DAYS: i64 = 7 * 86_400;
    window_minutes.map_or(SEVEN_DAYS, |m| SEVEN_DAYS.max(6 * m * 60))
}

/// The clean runs inside one Partition's intervals. Intervals belong to one run
/// while each begins exactly where the last ended, in time and in percentage; a
/// movement the evidence refused leaves a gap, and a gap is where a run ends.
fn runs_of(intervals: &[Interval]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut last: Option<&Interval> = None;
    for interval in intervals {
        let continues = last
            .is_some_and(|prev| prev.t1 == interval.t0 && prev.to_pct == interval.from_pct);
        match runs.last_mut() {
            Some(run) if continues => {
                run.tokens += interval.tokens;
                run.through = interval.t1;
                run.movement += interval.movement();
                run.positive_movements += 1;
            }
            _ => runs.push(Run {
                tokens: interval.tokens,
                from: interval.t0,
                through: interval.t1,
                movement: interval.movement(),
                positive_movements: 1,
            }),
        }
        last = Some(interval);
    }
    runs
}

/// The one run that speaks for a completed epoch: the qualifying one that spans
/// furthest. Two of equal span leave the epoch unable to say which is its own,
/// and an epoch that cannot say offers nothing rather than guessing.
fn representative(partition: &PartitionEvidence, estimate: &mut Estimate) -> Option<Candidate> {
    let qualifying: Vec<Run> =
        runs_of(&partition.intervals).into_iter().filter(Run::qualifies).collect();
    let Some(furthest) = qualifying.iter().map(|r| r.movement).max() else {
        estimate.refuse(ReasonCode::NoQualifyingRun);
        return None;
    };
    let mut widest = qualifying.iter().filter(|r| r.movement == furthest);
    let (Some(run), None) = (widest.next(), widest.next()) else {
        estimate.refuse(ReasonCode::AmbiguousGreatestRun);
        return None;
    };
    Some(Candidate {
        epoch_ended_at: partition.epoch,
        tokens: run.tokens,
        movement: run.movement,
        positive_movements: run.positive_movements,
        from: run.from,
        through: run.through,
        ratio: run.ratio(),
        quantization: run.quantization(),
    })
}

/// The narrowest and widest ratio among these candidates.
pub fn ratio_range(members: &[&Candidate]) -> Option<(f64, f64)> {
    let min = members.iter().map(|c| c.ratio).reduce(f64::min)?;
    let max = members.iter().map(|c| c.ratio).reduce(f64::max)?;
    Some((min, max))
}

/// Where every candidate's endpoint rounding could agree — `None` when they
/// agree nowhere, or when there are no candidates to agree. A member unbounded
/// above constrains nothing, so an upper of `None` here means unbounded rather
/// than absent.
pub fn quantization_intersection(members: &[&Candidate]) -> Option<Quantization> {
    let lower = members.iter().map(|c| c.quantization.lower).reduce(f64::max)?;
    let upper = members.iter().filter_map(|c| c.quantization.upper).reduce(f64::min);
    match upper {
        Some(upper) if lower > upper => None,
        upper => Some(Quantization { lower, upper }),
    }
}

/// Whether these candidates could be one Limit's constant ratio: their
/// endpoint-rounding ranges all overlap somewhere, and the widest ratio is no
/// more than a quarter again the narrowest.
fn coheres(members: &[&Candidate]) -> Option<ReasonCode> {
    if quantization_intersection(members).is_none() && !members.is_empty() {
        return Some(ReasonCode::QuantizationRangesDisjoint);
    }
    match ratio_range(members) {
        Some((min, max)) if max / min > 1.25 => Some(ReasonCode::RatioSpreadExceeded),
        _ => None,
    }
}

/// The conventional median, and with an even count the mean of the two middle
/// ratios. Full precision throughout: rounding belongs to the display.
fn median(mut ratios: Vec<f64>) -> f64 {
    ratios.sort_by(f64::total_cmp);
    let middle = ratios.len() / 2;
    if ratios.len() % 2 == 1 {
        ratios[middle]
    } else {
        (ratios[middle - 1] + ratios[middle]) / 2.0
    }
}

/// The estimate for every Series these Partitions belong to.
///
/// `evaluated_at` is the single evaluation instant, injected rather than read:
/// the same evidence at the same instant always gives the same answer, which is
/// what makes this testable without a clock and reconstructible afterwards.
pub fn estimates(
    partitions: &[PartitionEvidence],
    evaluated_at: i64,
) -> Vec<(SeriesKey, Estimate)> {
    let mut by_series: BTreeMap<&SeriesKey, Vec<&PartitionEvidence>> = BTreeMap::new();
    for partition in partitions {
        by_series.entry(&partition.series).or_default().push(partition);
    }

    by_series
        .into_iter()
        .map(|(series, mut epochs)| {
            let mut estimate = Estimate::default();

            // An epoch trains nothing until it is over: while a window is still
            // filling, what it will hold is not yet a fact.
            epochs.retain(|p| p.epoch <= evaluated_at);
            epochs.sort_by_key(|p| std::cmp::Reverse(p.epoch));

            // The newest epoch that names a window, not merely the newest: one
            // Reading short of a duration must not collapse a weekly Series'
            // horizon to the seven-day floor.
            let horizon = recency_horizon(epochs.iter().find_map(|p| p.window_minutes));
            estimate.horizon = horizon;
            let cutoff = evaluated_at - horizon;

            // The newest five *representatives*, not the newest five epochs: an
            // epoch that offers none has nothing to inspect, and letting it eat
            // a slot would report too few candidates while qualifying epochs sat
            // unread inside the horizon. Laziness stops the walk at the fifth,
            // so nothing further back is inspected or refused.
            let candidates: Vec<Candidate> = epochs
                .iter()
                .filter(|p| p.epoch >= cutoff)
                .filter_map(|p| representative(p, &mut estimate))
                .take(5)
                .collect();

            if candidates.len() < 3 {
                estimate.refuse(ReasonCode::InsufficientRecentEpochs);
                estimate.candidates = candidates;
                return (series.clone(), estimate);
            }

            // Every subset large enough to be a core, widest first.
            let n = candidates.len();
            // `ceil(0.75 * N)`, in integers: 3/3, 3/4, 4/5.
            let floor = 3.max((3 * n).div_ceil(4));
            let mut cores: Vec<Vec<usize>> = Vec::new();
            let mut reason: Option<ReasonCode> = None;
            for size in (floor..=n).rev() {
                for mask in 0u32..(1 << n) {
                    if (mask.count_ones() as usize) != size {
                        continue;
                    }
                    let members: Vec<usize> = (0..n).filter(|i| mask & (1 << i) != 0).collect();
                    let picked: Vec<&Candidate> = members.iter().map(|&i| &candidates[i]).collect();
                    match coheres(&picked) {
                        None => cores.push(members),
                        Some(why) => reason = reason.or(Some(why)),
                    }
                }
                if !cores.is_empty() {
                    break;
                }
            }

            match cores.len() {
                // Exactly one subset at the widest qualifying size is a core.
                1 => {
                    let core = cores.remove(0);
                    let ratios = core.iter().map(|&i| candidates[i].ratio).collect();
                    estimate.tokens_per_pct = Some(median(ratios));
                    estimate.core = core;
                }
                0 => {
                    if let Some(why) = reason {
                        estimate.refuse(why);
                    }
                }
                // More than one, and nothing chooses between them.
                _ => estimate.refuse(ReasonCode::CompetingStableCores),
            }
            estimate.candidates = candidates;
            (series.clone(), estimate)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-08-14T00:00:00Z, and the week before it.
    const NOW: i64 = 1_786_752_000;
    const DAY: i64 = 86_400;
    const WEEKLY: Option<i64> = Some(10_080);

    /// An interval that moved `movement` points and carried `tokens`.
    fn interval(from_pct: i64, movement: i64, tokens: i64, t0: i64) -> Interval {
        Interval { from_pct, to_pct: from_pct + movement, tokens, t0, t1: t0 + 60 }
    }

    /// Intervals that chain: each starts where the last ended, in time and in
    /// percentage, which is what makes them one run.
    fn chain(from_pct: i64, steps: &[(i64, i64)], t0: i64) -> Vec<Interval> {
        let mut out = Vec::new();
        let (mut pct, mut at) = (from_pct, t0);
        for (movement, tokens) in steps {
            out.push(Interval { from_pct: pct, to_pct: pct + movement, tokens: *tokens, t0: at, t1: at + 60 });
            pct += movement;
            at += 60;
        }
        out
    }

    fn partition(epoch_ended_at: i64, intervals: Vec<Interval>) -> PartitionEvidence {
        PartitionEvidence {
            series: series(),
            epoch: epoch_ended_at,
            window_minutes: WEEKLY,
            intervals,
        }
    }

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

    /// One qualifying run of `movement` points carrying `tokens`, as its own
    /// completed epoch ending `days_ago` before the evaluation instant.
    fn epoch(days_ago: i64, movement: i64, tokens: i64) -> PartitionEvidence {
        let half = movement / 2;
        partition(
            NOW - days_ago * DAY,
            chain(0, &[(half, tokens / 2), (movement - half, tokens - tokens / 2)], NOW - days_ago * DAY - 3_600),
        )
    }

    fn refused(estimate: &Estimate, reason: ReasonCode) -> usize {
        estimate.rejections.get(&reason).copied().unwrap_or(0)
    }

    fn only(estimates: Vec<(SeriesKey, Estimate)>) -> Estimate {
        assert_eq!(estimates.len(), 1, "one Series in these fixtures");
        estimates.into_iter().next().unwrap().1
    }

    #[test]
    fn a_run_ratio_is_its_tokens_over_its_movement_with_the_quantization_it_implies() {
        // The specification's own worked example.
        let run =
            Run { tokens: 1_000_000, from: 0, through: 60, movement: 10, positive_movements: 2 };
        assert_eq!(run.ratio(), 100_000.0);
        let quantization = run.quantization();
        assert_eq!(quantization.lower, 1_000_000.0 / 11.0);
        assert_eq!(quantization.upper, Some(1_000_000.0 / 9.0));
    }

    #[test]
    fn a_single_point_run_has_no_upper_quantization_bound() {
        // `T / (d - 1)` is unbounded at one point, and unbounded is not a number
        // the wire may carry as infinity.
        let run = Run { tokens: 500, from: 0, through: 60, movement: 1, positive_movements: 1 };
        assert_eq!(run.quantization().upper, None);
    }

    #[test]
    fn intervals_that_do_not_chain_are_different_runs() {
        // A refused movement between two stretches leaves a gap; the two sides
        // are not one monotonic sequence.
        let mut intervals = chain(0, &[(6, 600), (6, 600)], NOW - 3_600);
        intervals.push(interval(40, 6, 600, NOW - 1_000));
        let runs = runs_of(&intervals);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].movement, runs[0].tokens), (12, 1_200));
        assert_eq!((runs[1].movement, runs[1].tokens), (6, 600));
    }

    #[test]
    fn an_epoch_needs_two_movements_and_ten_points_to_offer_a_representative() {
        // One movement of twelve points: enough span, not enough movements.
        let one_movement = only(estimates(&[partition(NOW - DAY, chain(0, &[(12, 1_200)], NOW - 3_600))], NOW));
        assert_eq!(one_movement.candidates.len(), 0);
        assert_eq!(refused(&one_movement, ReasonCode::NoQualifyingRun), 1);

        // Two movements of nine points: enough movements, not enough span.
        let short = only(estimates(&[partition(NOW - DAY, chain(0, &[(4, 400), (5, 500)], NOW - 3_600))], NOW));
        assert_eq!(short.candidates.len(), 0);
        assert_eq!(refused(&short, ReasonCode::NoQualifyingRun), 1);

        // Two movements of ten points qualifies.
        let enough = only(estimates(&[partition(NOW - DAY, chain(0, &[(5, 500), (5, 500)], NOW - 3_600))], NOW));
        assert_eq!(enough.candidates.len(), 1);
    }

    #[test]
    fn an_epoch_offers_its_greatest_span_run_and_refuses_a_tie() {
        // Two qualifying runs, one plainly longer: the longer one represents.
        let mut intervals = chain(0, &[(5, 500), (5, 500)], NOW - 7_200);
        intervals.extend(chain(40, &[(10, 2_000), (10, 2_000)], NOW - 3_600));
        let chosen = only(estimates(&[partition(NOW - DAY, intervals)], NOW));
        assert_eq!(chosen.candidates.len(), 1);
        assert_eq!(chosen.candidates[0].movement, 20);

        // Two qualifying runs of equal span: the epoch cannot say which is its
        // representative, so it offers none at all.
        let mut tied = chain(0, &[(5, 500), (5, 500)], NOW - 7_200);
        tied.extend(chain(40, &[(5, 900), (5, 900)], NOW - 3_600));
        let ambiguous = only(estimates(&[partition(NOW - DAY, tied)], NOW));
        assert_eq!(ambiguous.candidates.len(), 0);
        assert_eq!(refused(&ambiguous, ReasonCode::AmbiguousGreatestRun), 1);
    }

    #[test]
    fn the_active_epoch_never_trains_the_estimate() {
        // Its reset instant has not passed, so the window is still filling and
        // what it will hold is not yet a fact.
        let active = only(estimates(&[epoch(-1, 20, 2_000)], NOW));
        assert_eq!(active.candidates.len(), 0);
    }

    #[test]
    fn recency_is_seven_days_or_six_windows_whichever_is_longer() {
        // A weekly window: six of them is 42 days, so a 30-day-old epoch is
        // recent even though seven days alone would have aged it out.
        let weekly = only(estimates(
            &[epoch(30, 20, 2_000), epoch(31, 20, 2_000), epoch(32, 20, 2_000)],
            NOW,
        ));
        assert_eq!(weekly.candidates.len(), 3);

        // With no duration at all, seven days is the horizon.
        let mut unknown: Vec<PartitionEvidence> =
            [30, 31, 32].iter().map(|d| epoch(*d, 20, 2_000)).collect();
        for partition in &mut unknown {
            partition.window_minutes = None;
        }
        assert_eq!(only(estimates(&unknown, NOW)).candidates.len(), 0);
    }

    #[test]
    fn no_more_than_the_newest_five_epochs_are_inspected() {
        let epochs: Vec<PartitionEvidence> =
            (1..=8).map(|d| epoch(d, 20, 2_000 + d * 10)).collect();
        let estimate = only(estimates(&epochs, NOW));
        assert_eq!(estimate.candidates.len(), 5);
        // The newest five, which are the ones nearest the evaluation instant.
        assert_eq!(estimate.candidates[0].epoch_ended_at, NOW - DAY);
        assert_eq!(estimate.candidates[4].epoch_ended_at, NOW - 5 * DAY);
    }

    #[test]
    fn an_epoch_with_nothing_to_offer_does_not_use_up_a_slot() {
        // Six recent epochs, the second-newest barren. Five representatives are
        // there to be had, and counting epochs instead of representatives would
        // report four — or, with enough barren ones, report too few candidates
        // while qualifying epochs sat unread inside the horizon.
        let mut epochs: Vec<PartitionEvidence> =
            (1..=6).map(|d| epoch(d, 20, 2_000 + d * 5)).collect();
        epochs[1] = partition(NOW - 2 * DAY, chain(0, &[(12, 1_200)], NOW - 2 * DAY - 3_600));

        let estimate = only(estimates(&epochs, NOW));
        assert_eq!(estimate.candidates.len(), 5);
        assert_eq!(refused(&estimate, ReasonCode::NoQualifyingRun), 1);
        // And the walk stopped at the fifth: the sixth epoch was never inspected.
        assert_eq!(estimate.candidates[4].epoch_ended_at, NOW - 6 * DAY);
    }

    #[test]
    fn a_horizon_comes_from_the_newest_epoch_that_names_a_window() {
        // The newest epoch happens not to name its duration. The Series is still
        // weekly, and collapsing it to the seven-day floor would age out
        // candidates that are not old.
        let mut epochs: Vec<PartitionEvidence> =
            [1, 30, 31, 32].iter().map(|d| epoch(*d, 20, 2_000)).collect();
        epochs[0].window_minutes = None;

        let estimate = only(estimates(&epochs, NOW));
        assert_eq!(estimate.candidates.len(), 4);
    }

    #[test]
    fn a_candidate_carries_what_its_contributors_can_be_found_by() {
        let estimate = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 2_020), epoch(3, 20, 1_980)],
            NOW,
        ));
        let newest = &estimate.candidates[0];
        assert_eq!(newest.positive_movements, 2);
        // The stretch itself: with the Series, exactly the Records inside it are
        // the contributors, which is what keeps them reconstructible without
        // being carried.
        assert!(newest.from < newest.through);
        assert_eq!(newest.through - newest.from, 120);
    }

    #[test]
    fn three_recent_epochs_are_the_fewest_that_can_carry_a_core() {
        let two = only(estimates(&[epoch(1, 20, 2_000), epoch(2, 20, 2_000)], NOW));
        assert_eq!(two.tokens_per_pct, None);
        assert_eq!(refused(&two, ReasonCode::InsufficientRecentEpochs), 1);
    }

    #[test]
    fn three_of_three_agreeing_epochs_are_a_core() {
        let estimate = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 2_020), epoch(3, 20, 1_980)],
            NOW,
        ));
        assert_eq!(estimate.core.len(), 3);
        // Ratios 100, 101, 99 → median 100.
        assert_eq!(estimate.tokens_per_pct, Some(100.0));
    }

    #[test]
    fn a_core_may_leave_one_epoch_out_of_four_but_not_two() {
        // 3 of 4 is the floor at N = 4: ceil(0.75 * 4) = 3.
        let three_of_four = only(estimates(
            &[
                epoch(1, 20, 2_000),
                epoch(2, 20, 2_020),
                epoch(3, 20, 1_980),
                epoch(4, 20, 8_000),
            ],
            NOW,
        ));
        assert_eq!(three_of_four.core.len(), 3);
        assert_eq!(three_of_four.tokens_per_pct, Some(100.0));

        // 4 of 5 is the floor at N = 5: ceil(0.75 * 5) = 4.
        let four_of_five = only(estimates(
            &[
                epoch(1, 20, 2_000),
                epoch(2, 20, 2_020),
                epoch(3, 20, 1_980),
                epoch(4, 20, 2_010),
                epoch(5, 20, 8_000),
            ],
            NOW,
        ));
        assert_eq!(four_of_five.core.len(), 4);
    }

    #[test]
    fn the_quantization_rule_binds_before_the_spread_rule_ever_can() {
        // Ratios 100 and 124 are inside the 1.25 spread, and still no core: at
        // twenty points of movement the endpoint-rounding ranges are ±5%, and
        // two ratios that far apart have no rounding in common.
        //
        // That is not a quirk of this fixture. A qualifying run moves at least
        // ten points, so its range is at worst `[T/11, T/9]`; two ranges overlap
        // only while the wider ratio is under `(d+1)/(d-1)` times the narrower,
        // which at ten points is 1.222 and tightens as runs lengthen. The
        // intersection test is therefore always the stricter of the two, and the
        // 1.25 spread is the outer guard the specification asks for rather than
        // the rule that usually decides.
        let estimate = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 2_480), epoch(3, 20, 2_200)],
            NOW,
        ));
        assert_eq!(estimate.tokens_per_pct, None);
        assert_eq!(refused(&estimate, ReasonCode::QuantizationRangesDisjoint), 1);

        // Close enough to share a rounding, and a core.
        let together = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 2_060), epoch(3, 20, 2_030)],
            NOW,
        ));
        assert_eq!(together.core.len(), 3);
    }

    #[test]
    fn the_spread_rule_refuses_what_the_ranges_would_have_allowed() {
        // The guard itself, on candidates whose ranges cannot decide: unbounded
        // above, so only the spread is left to judge them.
        let candidate = |ratio: f64| Candidate {
            epoch_ended_at: NOW,
            tokens: ratio as i64,
            movement: 1,
            positive_movements: 1,
            from: 0,
            through: 60,
            ratio,
            quantization: Quantization { lower: 0.0, upper: None },
        };
        let inside = [candidate(100.0), candidate(124.0), candidate(110.0)];
        assert_eq!(coheres(&inside.iter().collect::<Vec<_>>()), None);

        let outside = [candidate(100.0), candidate(126.0), candidate(110.0)];
        assert_eq!(
            coheres(&outside.iter().collect::<Vec<_>>()),
            Some(ReasonCode::RatioSpreadExceeded),
        );
    }

    #[test]
    fn ranges_that_never_overlap_are_no_core() {
        // Long runs quantize tightly, so ratios this far apart cannot share an
        // endpoint-rounding explanation even though their spread is inside 1.25.
        let estimate = only(estimates(
            &[epoch(1, 100, 10_000), epoch(2, 100, 11_500), epoch(3, 100, 12_400)],
            NOW,
        ));
        assert_eq!(estimate.tokens_per_pct, None);
        assert_eq!(refused(&estimate, ReasonCode::QuantizationRangesDisjoint), 1);
    }

    #[test]
    fn two_cores_of_equal_size_leave_no_unique_one() {
        // Four candidates at ten points each, ratios 100, 112, 89, 100. Two
        // different threes cohere — {100, 112, 100} and {100, 89, 100} — and
        // nothing chooses between them, so there is no core rather than a
        // coin-toss core.
        let estimate = only(estimates(
            &[
                epoch(1, 10, 1_000),
                epoch(2, 10, 1_120),
                epoch(3, 10, 890),
                epoch(4, 10, 1_000),
            ],
            NOW,
        ));
        assert_eq!(estimate.tokens_per_pct, None);
        assert_eq!(refused(&estimate, ReasonCode::CompetingStableCores), 1);
    }

    #[test]
    fn candidates_that_agree_nowhere_are_no_core_either() {
        // Three ratios too far apart for any qualifying subset: not competing
        // cores, simply none.
        let estimate = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 6_000), epoch(3, 20, 12_000)],
            NOW,
        ));
        assert_eq!(estimate.tokens_per_pct, None);
        assert_eq!(refused(&estimate, ReasonCode::CompetingStableCores), 0);
    }

    #[test]
    fn an_even_core_takes_the_mean_of_the_two_middle_ratios() {
        // Four members, ratios 100, 102, 104, 106 → (102 + 104) / 2.
        let estimate = only(estimates(
            &[
                epoch(1, 20, 2_000),
                epoch(2, 20, 2_040),
                epoch(3, 20, 2_080),
                epoch(4, 20, 2_120),
            ],
            NOW,
        ));
        assert_eq!(estimate.core.len(), 4);
        assert_eq!(estimate.tokens_per_pct, Some(103.0));
    }

    #[test]
    fn the_median_is_of_whole_run_ratios_never_of_pooled_tokens() {
        // Pooling would give (2_000 + 2_000 + 20_000) / (20 + 20 + 200) = 100.
        // The median of the three run ratios is 100 as well — so make the runs
        // disagree in span but agree in ratio, and check the odd one out cannot
        // drag the answer the way a pooled sum would.
        let estimate = only(estimates(
            &[epoch(1, 20, 2_000), epoch(2, 20, 2_020), epoch(3, 100, 9_900)],
            NOW,
        ));
        // Ratios 100, 101, 99: the median is the middle ratio, not the weighted
        // average the long run would dominate.
        assert_eq!(estimate.tokens_per_pct, Some(100.0));
    }
}

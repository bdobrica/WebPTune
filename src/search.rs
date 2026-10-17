//! Heuristic or exhaustive search over integer lossy qualities and lossless.

use std::fmt;

use anyhow::{ensure, Context, Result};
use clap::ValueEnum;

use crate::encoder::{Mode, TrialEvaluator};

/// A candidate is the evaluator's encoded, decoded and measured trial.
/// Reuse that type so candidate bytes cannot bypass the evaluator in this API.
pub use crate::encoder::Trial as Candidate;

const MAX_BINARY_STEPS: usize = 7;
const BOUNDARY_RADIUS: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum SearchMode {
    /// Bounded binary search and nearby probes; at most 19 measured trials
    #[default]
    Heuristic,
    /// All 101 integer lossy qualities plus lossless; 102 measured trials
    Exhaustive,
}

#[derive(Clone, Copy, Debug)]
pub struct SearchOptions {
    /// Lossy evaluations, including both endpoints; valid range is 2..=101.
    /// Lossless always gets one additional evaluation outside this budget.
    /// The default 18 permits endpoints, seven binary steps and nine probes,
    /// before deduplication. Smaller budgets can truncate the neighborhood.
    pub max_lossy_trials: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_lossy_trials: 18,
        }
    }
}

/// Numeric history only: inferior candidates' encoded buffers are discarded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrialSummary {
    pub mode: Mode,
    pub score: f64,
    pub size: usize,
}

impl TrialSummary {
    pub fn is_feasible(&self, threshold: f64) -> bool {
        self.score >= threshold
    }

    // Score affects feasibility only. Equal sizes prefer lossy, then lower q.
    fn ordering_key(&self) -> (usize, u8) {
        let rank = match self.mode {
            Mode::Lossy { quality } => quality,
            Mode::Lossless => 101,
        };
        (self.size, rank)
    }
}

impl From<&Candidate> for TrialSummary {
    fn from(candidate: &Candidate) -> Self {
        Self {
            mode: candidate.mode(),
            score: candidate.score(),
            size: candidate.size(),
        }
    }
}

#[derive(Debug)]
pub struct SearchResult {
    candidate: Candidate,
    trials: Vec<TrialSummary>,
}

impl SearchResult {
    pub fn candidate(&self) -> &Candidate {
        &self.candidate
    }

    /// Actual evaluation order, with no duplicate lossy qualities.
    pub fn trials(&self) -> &[TrialSummary] {
        &self.trials
    }

    pub fn into_candidate(self) -> Candidate {
        self.candidate
    }
}

/// No tested candidate passed. The mode determines whether qualities were skipped.
#[derive(Debug)]
pub struct ThresholdUnachievable {
    pub threshold: f64,
    pub best_score: f64,
    pub evaluated_candidates: usize,
    pub search_mode: SearchMode,
}

impl fmt::Display for ThresholdUnachievable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "no tested candidate satisfies SSIMULACRA2 threshold {} (best measured score {}; {} trials)",
            self.threshold, self.best_score, self.evaluated_candidates
        )?;
        match self.search_mode {
            SearchMode::Heuristic => write!(formatter, "; heuristic search may miss untested feasible qualities"),
            SearchMode::Exhaustive => write!(formatter, "; exhaustive search evaluated all 101 integer lossy qualities and lossless with the current encoder settings"),
        }
    }
}

impl std::error::Error for ThresholdUnachievable {}

/// Find the smallest tested feasible bytes using the default bounded heuristic.
/// No global optimum or monotonic score/size behavior is assumed or promised.
pub fn search(evaluator: &TrialEvaluator<'_>, threshold: f64) -> Result<SearchResult> {
    search_with_options(evaluator, threshold, SearchOptions::default())
}

/// Exhaustive mode chooses the smallest feasible result across all 101 integer
/// lossy qualities and lossless, with the evaluator's fixed encoder settings.
/// This does not cover fractional qualities or other encoder configurations.
/// Either mode aborts on an evaluation error without returning a partial result.
pub fn search_with_mode(
    evaluator: &TrialEvaluator<'_>,
    threshold: f64,
    mode: SearchMode,
) -> Result<SearchResult> {
    search_using(threshold, mode, SearchOptions::default(), |mode| {
        evaluator.evaluate(mode)
    })
}

/// Run the heuristic with a custom lossy evaluation budget.
pub fn search_with_options(
    evaluator: &TrialEvaluator<'_>,
    threshold: f64,
    options: SearchOptions,
) -> Result<SearchResult> {
    search_with(threshold, options, |mode| evaluator.evaluate(mode))
}

// The injected evaluator lets unit tests exercise nonmonotonic curves and
// codec failures deterministically, without weakening the public Candidate API.
fn search_with(
    threshold: f64,
    options: SearchOptions,
    evaluate: impl FnMut(Mode) -> Result<Candidate>,
) -> Result<SearchResult> {
    search_using(threshold, SearchMode::Heuristic, options, evaluate)
}

fn search_using(
    threshold: f64,
    mode: SearchMode,
    mut options: SearchOptions,
    evaluate: impl FnMut(Mode) -> Result<Candidate>,
) -> Result<SearchResult> {
    ensure!(
        threshold.is_finite() && (0.0..=100.0).contains(&threshold),
        "threshold must be a finite number in [0, 100]"
    );
    ensure!(
        (2..=101).contains(&options.max_lossy_trials),
        "lossy evaluation budget must be in 2..=101"
    );
    if mode == SearchMode::Exhaustive {
        options.max_lossy_trials = 101;
    }
    let mut state = SearchState {
        threshold,
        options,
        evaluate,
        tested: [None; 101],
        lossy_trials: 0,
        trials: Vec::new(),
        best: None,
    };
    match mode {
        SearchMode::Heuristic => heuristic(&mut state)?,
        SearchMode::Exhaustive => {
            for quality in 0..=100 {
                state
                    .lossy(quality)?
                    .expect("exhaustive budget covers every integer quality");
            }
        }
    }
    // Never assume lossless is feasible, or skip it because lossy passed.
    state.evaluate(Mode::Lossless)?;

    let candidate = state.best.ok_or_else(|| ThresholdUnachievable {
        threshold,
        best_score: state
            .trials
            .iter()
            .map(|trial| trial.score)
            .fold(f64::NEG_INFINITY, f64::max),
        evaluated_candidates: state.trials.len(),
        search_mode: mode,
    })?;
    Ok(SearchResult {
        candidate,
        trials: state.trials,
    })
}

fn heuristic(state: &mut SearchState<impl FnMut(Mode) -> Result<Candidate>>) -> Result<()> {
    let zero = state.lossy(0)?.expect("budget includes both endpoints");
    state.lossy(100)?;

    let mut boundary = 0;
    if !zero.is_feasible(state.threshold) {
        boundary = 100;
        let (mut low, mut high) = (1, 99);
        // Run even if q=100 fails: its score cannot rule out lower qualities.
        for _ in 0..MAX_BINARY_STEPS {
            if low > high {
                break;
            }
            let quality = low + (high - low) / 2;
            let Some(trial) = state.lossy(quality)? else {
                break;
            };
            if trial.is_feasible(state.threshold) {
                boundary = quality;
                high = quality - 1;
            } else {
                low = quality + 1;
            }
        }
    }
    for quality in
        boundary.saturating_sub(BOUNDARY_RADIUS)..=boundary.saturating_add(BOUNDARY_RADIUS).min(100)
    {
        if state.lossy(quality)?.is_none() {
            break;
        }
    }
    Ok(())
}

struct SearchState<F> {
    threshold: f64,
    options: SearchOptions,
    evaluate: F,
    tested: [Option<TrialSummary>; 101],
    lossy_trials: usize,
    trials: Vec<TrialSummary>,
    best: Option<Candidate>,
}

impl<F: FnMut(Mode) -> Result<Candidate>> SearchState<F> {
    fn lossy(&mut self, quality: u8) -> Result<Option<TrialSummary>> {
        if let Some(cached) = self.tested[usize::from(quality)] {
            return Ok(Some(cached));
        }
        if self.lossy_trials >= self.options.max_lossy_trials {
            return Ok(None);
        }
        let trial = self.evaluate(Mode::Lossy { quality })?;
        self.tested[usize::from(quality)] = Some(trial);
        self.lossy_trials += 1;
        Ok(Some(trial))
    }

    fn evaluate(&mut self, mode: Mode) -> Result<TrialSummary> {
        let candidate = (self.evaluate)(mode)
            .with_context(|| format!("cannot evaluate search candidate {mode:?}"))?;
        let trial = TrialSummary::from(&candidate);
        if trial.is_feasible(self.threshold)
            && self
                .best
                .as_ref()
                .is_none_or(|best| trial.ordering_key() < TrialSummary::from(best).ordering_key())
        {
            self.best = Some(candidate);
        }
        self.trials.push(trial);
        Ok(trial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        threshold: f64,
        mut lossy: impl FnMut(u8) -> (f64, usize),
        lossless: (f64, usize),
    ) -> Result<SearchResult> {
        search_with(threshold, SearchOptions::default(), |mode| {
            let (score, size) = match mode {
                Mode::Lossy { quality } => lossy(quality),
                Mode::Lossless => lossless,
            };
            Ok(Candidate::synthetic(mode, score, size))
        })
    }

    fn qualities(result: &SearchResult) -> Vec<u8> {
        result
            .trials()
            .iter()
            .filter_map(|trial| match trial.mode {
                Mode::Lossy { quality } => Some(quality),
                Mode::Lossless => None,
            })
            .collect()
    }

    #[test]
    fn threshold_is_inclusive_without_an_epsilon() {
        let result = run(
            80.0,
            |q| {
                if q == 100 {
                    (80.0, 10)
                } else {
                    (79.999_999_999, 1)
                }
            },
            (79.999_999_999, 1),
        )
        .unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossy { quality: 100 });
        assert_eq!(result.candidate().score(), 80.0);
        assert_eq!(result.candidate().size(), 10);
    }

    #[test]
    fn smallest_bytes_win_even_at_the_highest_quality() {
        let result = run(80.0, |q| (80.0, 110 - usize::from(q)), (100.0, 200)).unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossy { quality: 100 });
        assert_eq!(result.candidate().size(), 10);
        // A passing q=0 still gets its clipped neighborhood and q=100.
        assert_eq!(qualities(&result), [0, 100, 1, 2, 3, 4]);
    }

    #[test]
    fn lossless_is_always_measured_and_can_be_smaller_than_passing_lossy() {
        let result = run(80.0, |_| (90.0, 20), (80.0, 10)).unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossless);
        assert_eq!(result.candidate().score(), 80.0);
        assert_eq!(result.trials().last().unwrap().mode, Mode::Lossless);
        assert_eq!(
            result
                .trials()
                .iter()
                .filter(|trial| trial.mode == Mode::Lossless)
                .count(),
            1
        );
        assert_eq!(result.into_candidate().into_bytes(), vec![0; 10]);
    }

    #[test]
    fn unachievable_threshold_is_a_typed_failure_including_measured_lossless() {
        for (threshold, score) in [(100.0, 99.999_999_999), (0.0, -1.0)] {
            let error = run(threshold, |_| (-20.0, 1), (score, 1)).unwrap_err();
            let failure = error.downcast_ref::<ThresholdUnachievable>().unwrap();
            assert_eq!(failure.threshold, threshold);
            assert_eq!(failure.best_score, score);
            assert!((3..=19).contains(&failure.evaluated_candidates));
            assert!(error.to_string().contains("no tested candidate satisfies"));
            assert!(error.to_string().contains("untested feasible qualities"));
        }
    }

    #[test]
    fn nonmonotonic_scores_and_sizes_choose_best_tested_not_binary_boundary() {
        let result = run(
            80.0,
            |q| match q {
                50 => (80.0, 10),
                52 => (81.0, 5),
                54 => (90.0, 20),
                _ => (-10.0, 1),
            },
            (100.0, 30),
        )
        .unwrap();
        // Both endpoints fail; binary search still discovers q=50. Probes
        // recover q=52 across a failing q=51, and q=54 has a worse byte size.
        assert_eq!(result.candidate().mode(), Mode::Lossy { quality: 52 });
        assert_eq!(result.candidate().size(), 5);
        for q in 46..=54 {
            assert!(qualities(&result).contains(&q));
        }
    }

    #[test]
    fn heuristic_can_miss_a_feasible_quality_outside_its_probes() {
        let result = run(80.0, |q| (if q == 17 { 80.0 } else { 0.0 }, 1), (100.0, 10)).unwrap();
        assert!(!qualities(&result).contains(&17));
        assert_eq!(result.candidate().mode(), Mode::Lossless);
        // With no passing lossy trial, probe the upper endpoint neighborhood.
        for q in 96..=100 {
            assert!(qualities(&result).contains(&q));
        }
    }

    fn exhaustive(
        threshold: f64,
        evaluate: impl FnMut(Mode) -> Result<Candidate>,
    ) -> Result<SearchResult> {
        search_using(
            threshold,
            SearchMode::Exhaustive,
            SearchOptions::default(),
            evaluate,
        )
    }

    #[test]
    fn exhaustive_finds_every_isolated_feasible_quality_including_endpoints() {
        for feasible in 0..=100 {
            let result = exhaustive(80.0, |mode| {
                let score = if mode == (Mode::Lossy { quality: feasible }) {
                    80.0
                } else {
                    79.999_999_999
                };
                Ok(Candidate::synthetic(mode, score, 10))
            })
            .unwrap();
            assert_eq!(result.candidate().mode(), Mode::Lossy { quality: feasible });
            assert_eq!(result.candidate().score(), 80.0);
            assert_eq!(qualities(&result), (0..=100).collect::<Vec<_>>());
            assert_eq!(result.trials().len(), 102);
            assert_eq!(result.trials().last().unwrap().mode, Mode::Lossless);
        }
    }

    #[test]
    fn exhaustive_selects_by_size_and_stable_ties_across_nonmonotonic_curves() {
        for lossless in [(100.0, 1), (100.0, 2), (79.0, 1)] {
            let result = exhaustive(80.0, |mode| {
                let (score, size) = match mode {
                    Mode::Lossy { quality: 17 | 23 } => (80.0, 2),
                    Mode::Lossy { quality: 99 } => (79.0, 1),
                    Mode::Lossy { quality } => (90.0, 10 + usize::from(quality % 7)),
                    Mode::Lossless => lossless,
                };
                Ok(Candidate::synthetic(mode, score, size))
            })
            .unwrap();
            assert_eq!(result.trials().len(), 102);
            assert_eq!(
                result.candidate().mode(),
                if lossless == (100.0, 1) {
                    Mode::Lossless
                } else {
                    Mode::Lossy { quality: 17 }
                }
            );
        }
    }

    #[test]
    fn exhaustive_failure_reports_the_complete_fixed_set_without_heuristic_caveat() {
        for (threshold, score) in [(100.0, 99.999_999_999), (0.0, -1.0)] {
            let mut modes = Vec::new();
            let error = exhaustive(threshold, |mode| {
                modes.push(mode);
                Ok(Candidate::synthetic(mode, score, 1))
            })
            .unwrap_err();
            let failure = error.downcast_ref::<ThresholdUnachievable>().unwrap();
            assert_eq!(failure.search_mode, SearchMode::Exhaustive);
            assert_eq!(failure.threshold, threshold);
            assert_eq!(failure.best_score, score);
            assert_eq!(failure.evaluated_candidates, 102);
            assert_eq!(modes.len(), 102);
            for quality in 0..=100 {
                assert_eq!(modes[usize::from(quality)], Mode::Lossy { quality });
            }
            assert_eq!(modes[101], Mode::Lossless);
            assert!(error
                .to_string()
                .contains("all 101 integer lossy qualities and lossless"));
            assert!(!error.to_string().contains("untested"));
            assert!(!error.to_string().contains("heuristic"));
        }
    }

    #[test]
    fn exhaustive_aborts_on_evaluation_errors_without_returning_a_partial_winner() {
        for failed_mode in [
            Mode::Lossy { quality: 0 },
            Mode::Lossy { quality: 17 },
            Mode::Lossy { quality: 100 },
            Mode::Lossless,
        ] {
            let mut modes = Vec::new();
            let error = exhaustive(80.0, |mode| {
                modes.push(mode);
                if mode == failed_mode {
                    anyhow::bail!("synthetic codec failure");
                }
                Ok(Candidate::synthetic(mode, 100.0, 10))
            })
            .unwrap_err();
            assert_eq!(modes.last(), Some(&failed_mode));
            let expected_calls = match failed_mode {
                Mode::Lossy { quality } => usize::from(quality) + 1,
                Mode::Lossless => 102,
            };
            assert_eq!(modes.len(), expected_calls);
            assert!(error.to_string().contains(&format!("{failed_mode:?}")));
            assert_eq!(error.root_cause().to_string(), "synthetic codec failure");
            assert!(error.downcast_ref::<ThresholdUnachievable>().is_none());
        }
    }

    #[test]
    fn exhaustive_rejects_invalid_thresholds_before_evaluation() {
        for threshold in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 100.1] {
            let error = exhaustive(threshold, |_| panic!("must not evaluate")).unwrap_err();
            assert!(error.to_string().contains("threshold must be"));
        }
    }

    #[test]
    fn default_budget_covers_boundary_neighborhood_and_deduplicates_all_curves() {
        for boundary in 0..=100_u8 {
            let result = run(
                80.0,
                |q| {
                    (
                        if q >= boundary { 80.0 } else { 79.0 },
                        // Deliberately nonmonotonic sizes among feasible q.
                        usize::from(q % 7) + 1,
                    )
                },
                (100.0, 100),
            )
            .unwrap();
            let tested = qualities(&result);
            assert_eq!(&tested[..2], [0, 100]);
            assert!(result.trials().len() <= 19);
            for q in boundary.saturating_sub(4)..=boundary.saturating_add(4).min(100) {
                assert!(tested.contains(&q), "boundary={boundary}, missing {q}");
            }
            let mut unique = tested.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(tested.len(), unique.len());
            let expected = result
                .trials()
                .iter()
                .filter(|trial| trial.is_feasible(80.0))
                .min_by_key(|trial| trial.ordering_key())
                .unwrap();
            assert_eq!(&TrialSummary::from(result.candidate()), expected);
        }
    }

    #[test]
    fn budgets_bound_lossy_trials_but_reserve_endpoints_and_lossless() {
        for budget in 2..=101 {
            let result = search_with(
                80.0,
                SearchOptions {
                    max_lossy_trials: budget,
                },
                |mode| {
                    let score = match mode {
                        Mode::Lossy { quality } if quality < 50 => 0.0,
                        _ => 100.0,
                    };
                    Ok(Candidate::synthetic(mode, score, 10))
                },
            )
            .unwrap();
            assert_eq!(&qualities(&result)[..2], [0, 100]);
            assert!(qualities(&result).len() <= budget);
            assert_eq!(result.trials().last().unwrap().mode, Mode::Lossless);
            if budget == 2 {
                assert_eq!(result.trials().len(), 3);
            }
        }
    }

    #[test]
    fn duplicate_requests_reuse_cache_even_when_budget_is_exhausted() {
        let mut calls = 0;
        let mut state = SearchState {
            threshold: 80.0,
            options: SearchOptions {
                max_lossy_trials: 2,
            },
            evaluate: |mode| {
                calls += 1;
                Ok(Candidate::synthetic(mode, 80.0, 10))
            },
            tested: [None; 101],
            lossy_trials: 0,
            trials: Vec::new(),
            best: None,
        };
        let first = state.lossy(7).unwrap();
        assert_eq!(state.lossy(7).unwrap(), first);
        state.lossy(8).unwrap();
        assert!(state.lossy(9).unwrap().is_none());
        assert_eq!(state.lossy(7).unwrap(), first);
        assert_eq!(state.trials.len(), 2);
        drop(state);
        assert_eq!(calls, 2);
    }

    #[test]
    fn equal_sizes_prefer_lossy_then_lower_quality_regardless_of_score() {
        let result = run(80.0, |q| (80.0 + f64::from(q) / 5.0, 10), (100.0, 10)).unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossy { quality: 0 });
        assert_eq!(result.candidate().score(), 80.0);
        // q=50 is evaluated before q=49; the later lower quality must replace it.
        let result = run(
            80.0,
            |q| (if q >= 49 { 80.0 } else { 0.0 }, 10),
            (100.0, 10),
        )
        .unwrap();
        assert_eq!(result.candidate().mode(), Mode::Lossy { quality: 49 });
    }

    #[test]
    fn invalid_thresholds_and_budgets_fail_before_evaluation() {
        for threshold in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 100.1] {
            let error = search_with(threshold, SearchOptions::default(), |_| {
                panic!("invalid threshold must not evaluate")
            })
            .unwrap_err();
            assert!(error.to_string().contains("threshold must be"));
        }
        for budget in [0, 1, 102, usize::MAX] {
            let error = search_with(
                80.0,
                SearchOptions {
                    max_lossy_trials: budget,
                },
                |_| panic!("invalid budget must not evaluate"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("budget must be"));
        }
    }

    #[test]
    fn evaluator_errors_propagate_instead_of_becoming_infeasible_trials() {
        for failed_mode in [Mode::Lossy { quality: 50 }, Mode::Lossless] {
            let error = search_with(80.0, SearchOptions::default(), |mode| {
                if mode == failed_mode {
                    anyhow::bail!("synthetic codec failure");
                }
                Ok(Candidate::synthetic(
                    mode,
                    if mode == (Mode::Lossy { quality: 0 }) {
                        0.0
                    } else {
                        100.0
                    },
                    10,
                ))
            })
            .unwrap_err();
            assert!(error.to_string().contains(&format!("{failed_mode:?}")));
            assert_eq!(error.root_cause().to_string(), "synthetic codec failure");
            assert!(error.downcast_ref::<ThresholdUnachievable>().is_none());
        }
    }
}

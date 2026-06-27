use std::cmp::Ordering;

pub use fuzzy_rank::path::typo::TypoQuery;

use crate::db::{Dir, Epoch};

#[derive(Clone, Copy, Debug)]
pub struct TypoMatch<'a> {
    pub dir: &'a Dir<'a>,
    pub distance: usize,
    pub ratio: f64,
    pub path_position: usize,
    pub structure: usize,
    inner: fuzzy_rank::path::typo::PathMatch<'a>,
}

impl<'a> TypoMatch<'a> {
    fn from_path_match(dir: &'a Dir<'a>, inner: fuzzy_rank::path::typo::PathMatch<'a>) -> Self {
        Self {
            dir,
            distance: inner.distance,
            ratio: inner.ratio,
            path_position: inner.path_position,
            structure: inner.structure,
            inner,
        }
    }

    fn cmp(&self, other: &Self) -> Ordering {
        self.inner.compare(&other.inner)
    }
}

pub fn best_match_dir<'a>(
    query: &TypoQuery,
    dir: &'a Dir<'a>,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    let score = dir.score(now);
    query
        .best_match(dir.path.as_ref(), score)
        .map(|candidate| TypoMatch::from_path_match(dir, candidate))
}

pub fn best_basename_match_dir<'a>(
    query: &TypoQuery,
    dir: &'a Dir<'a>,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    let score = dir.score(now);
    query
        .best_basename_match(dir.path.as_ref(), score)
        .map(|candidate| TypoMatch::from_path_match(dir, candidate))
}

pub fn sort_matches(matches: &mut [TypoMatch<'_>]) {
    matches.sort_unstable_by(TypoMatch::cmp);
}

pub fn is_ambiguous(first: &TypoMatch<'_>, second: &TypoMatch<'_>) -> bool {
    fuzzy_rank::path::typo::is_ambiguous(&first.inner, &second.inner)
}

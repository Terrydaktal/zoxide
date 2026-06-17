use std::cmp::Ordering;
use std::fmt::{self, Display, Formatter};

use crate::db::{Dir, Epoch, Rank};
use crate::util;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MatchScope {
    Basename = 0,
    BasenameToken = 1,
    OtherComponent = 2,
    OtherComponentToken = 3,
    FullPath = 4,
}

impl Display for MatchScope {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Basename => write!(f, "basename"),
            Self::BasenameToken => write!(f, "basename-token"),
            Self::OtherComponent => write!(f, "component"),
            Self::OtherComponentToken => write!(f, "component-token"),
            Self::FullPath => write!(f, "path"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TypoMatch<'a> {
    pub dir: &'a Dir<'a>,
    pub distance: usize,
    pub ratio: f64,
    pub scope: MatchScope,
    pub path_position: usize,
    pub structure: usize,
    score: Rank,
    path_depth: usize,
}

impl<'a> TypoMatch<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance
            .cmp(&other.distance)
            .then_with(|| self.ratio.total_cmp(&other.ratio))
            .then_with(|| self.scope.cmp(&other.scope))
            .then_with(|| self.path_position.cmp(&other.path_position))
            .then_with(|| self.structure.cmp(&other.structure))
            .then_with(|| other.score.total_cmp(&self.score))
            .then_with(|| self.path_depth.cmp(&other.path_depth))
            .then_with(|| self.dir.path.cmp(&other.dir.path))
    }
}

pub fn max_typos(len: usize) -> usize {
    len / 2
}

pub fn best_match<'a>(dir: &'a Dir<'a>, query: &str, now: Epoch) -> Option<TypoMatch<'a>> {
    if query.chars().count() <= 1 {
        return None;
    }

    let lower_path = util::to_lowercase(&dir.path);
    let components = path_components(&lower_path);
    if components.is_empty() {
        return None;
    }

    let basename = *components.last().unwrap();
    let basename_idx = components.len() - 1;
    let mut best = candidate_for_text(dir, query, basename, MatchScope::Basename, 0, now);
    update_best(
        &mut best,
        candidate_for_token_sequence(dir, query, basename, MatchScope::Basename, 0, now),
    );
    update_best(
        &mut best,
        candidate_for_compound_component(dir, query, basename, MatchScope::Basename, 0, now),
    );
    update_best(
        &mut best,
        candidate_for_component_sequence(dir, query, &components, MatchScope::OtherComponent, now),
    );

    for token in split_tokens(basename) {
        update_best(
            &mut best,
            candidate_for_text(dir, query, token, MatchScope::BasenameToken, 0, now),
        );
    }

    for (idx, component) in components[..components.len().saturating_sub(1)].iter().enumerate() {
        let path_position = basename_idx - idx;
        update_best(
            &mut best,
            candidate_for_text(
                dir,
                query,
                component,
                MatchScope::OtherComponent,
                path_position,
                now,
            ),
        );
        update_best(
            &mut best,
            candidate_for_compound_component(
                dir,
                query,
                component,
                MatchScope::OtherComponent,
                path_position,
                now,
            ),
        );
        for token in split_tokens(component) {
            update_best(
                &mut best,
                candidate_for_text(
                    dir,
                    query,
                    token,
                    MatchScope::OtherComponentToken,
                    path_position,
                    now,
                ),
            );
        }
    }

    update_best(
        &mut best,
        candidate_for_text(dir, query, &lower_path, MatchScope::FullPath, basename_idx + 1, now),
    );

    best
}

pub fn sort_matches(matches: &mut [TypoMatch<'_>]) {
    matches.sort_unstable_by(TypoMatch::cmp);
}

pub fn is_ambiguous(first: &TypoMatch<'_>, second: &TypoMatch<'_>) -> bool {
    first.distance == second.distance
        && first.scope == second.scope
        && first.path_position == second.path_position
        && first.structure == second.structure
        && (first.ratio - second.ratio).abs() <= 0.02
        && first.score.total_cmp(&second.score).is_eq()
}

fn update_best<'a>(best: &mut Option<TypoMatch<'a>>, candidate: Option<TypoMatch<'a>>) {
    match (best.as_ref(), candidate) {
        (_, None) => {}
        (None, Some(candidate)) => *best = Some(candidate),
        (Some(current), Some(candidate)) if candidate.cmp(current).is_lt() => {
            *best = Some(candidate)
        }
        _ => {}
    }
}

fn candidate_for_text<'a>(
    dir: &'a Dir<'a>,
    query: &str,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    if candidate.is_empty() {
        return None;
    }

    let query_len = query.chars().count();
    let candidate_len = candidate.chars().count();
    let limit = max_typos(query_len);
    if limit == 0 || query_len.abs_diff(candidate_len) > limit {
        return None;
    }

    let distance = bounded_damerau_levenshtein(query, candidate, limit)?;
    let max_len = query_len.max(candidate_len) as f64;
    let ratio = distance as f64 / max_len;
    if ratio > 0.5 {
        return None;
    }

    Some(TypoMatch {
        dir,
        distance,
        ratio,
        scope,
        path_position: path_position * 3,
        structure: 0,
        score: dir.score(now),
        path_depth: path_components(&dir.path).len(),
    })
}

fn candidate_for_token_sequence<'a>(
    dir: &'a Dir<'a>,
    query: &str,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    let query_tokens: Vec<_> = split_tokens(query).collect();
    let candidate_tokens: Vec<_> = split_tokens(candidate).collect();
    if query_tokens.len() < 2 || query_tokens.len() != candidate_tokens.len() {
        return None;
    }

    let query_len = query.chars().count();
    let limit = max_typos(query_len);
    let mut distance = 0;
    let mut path_metric = 0;
    let mut structure = 0;
    for (query_token, candidate_token) in query_tokens.into_iter().zip(candidate_tokens) {
        let remaining = limit.checked_sub(distance)?;
        let (cost, penalty, position_rank, _) =
            best_token_match(query_token, candidate_token, remaining)?;
        distance += cost;
        path_metric += path_position * 3 + position_rank;
        structure += penalty;
    }

    let ratio = distance as f64 / query_len as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(TypoMatch {
        dir,
        distance,
        ratio,
        scope,
        path_position: path_metric,
        structure,
        score: dir.score(now),
        path_depth: path_components(&dir.path).len(),
    })
}

fn candidate_for_compound_component<'a>(
    dir: &'a Dir<'a>,
    query: &str,
    candidate: &str,
    scope: MatchScope,
    path_position: usize,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    let query_tokens: Vec<_> = split_tokens(query).collect();
    let candidate_tokens: Vec<_> = split_tokens(candidate).collect();
    if query_tokens.len() < 2 || candidate_tokens.len() != 1 {
        return None;
    }

    let query_len = query.chars().count();
    let limit = max_typos(query_len);
    let (distance, structure, position_metric) =
        partitioned_token_distance(&query_tokens, candidate_tokens[0], limit)?;
    let ratio = distance as f64 / query_len.max(candidate.chars().count()) as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(TypoMatch {
        dir,
        distance,
        ratio,
        scope,
        path_position: path_position * 3 * query_tokens.len() + position_metric,
        structure,
        score: dir.score(now),
        path_depth: path_components(&dir.path).len(),
    })
}

fn candidate_for_component_sequence<'a>(
    dir: &'a Dir<'a>,
    query: &str,
    components: &[&str],
    scope: MatchScope,
    now: Epoch,
) -> Option<TypoMatch<'a>> {
    let query_tokens: Vec<_> = split_tokens(query).collect();
    if query_tokens.len() < 2 {
        return None;
    }

    let basename_idx = components.len() - 1;
    let candidate_tokens: Vec<_> = components
        .iter()
        .enumerate()
        .flat_map(|(idx, component)| {
            let path_position = basename_idx - idx;
            split_tokens(component).map(move |token| TokenCandidate { token, path_position })
        })
        .collect();
    if candidate_tokens.len() < query_tokens.len() {
        return None;
    }

    let query_len = query.chars().count();
    let limit = max_typos(query_len);
    let (distance, path_position, structure) =
        aligned_token_distance(&query_tokens, &candidate_tokens, limit)?;
    let ratio = distance as f64 / query_len as f64;
    if ratio > 0.5 {
        return None;
    }

    Some(TypoMatch {
        dir,
        distance,
        ratio,
        scope,
        path_position,
        structure,
        score: dir.score(now),
        path_depth: path_components(&dir.path).len(),
    })
}

fn partitioned_token_distance(
    query_tokens: &[&str],
    candidate: &str,
    limit: usize,
) -> Option<(usize, usize, usize)> {
    let mut boundaries: Vec<usize> = candidate.char_indices().map(|(idx, _)| idx).collect();
    boundaries.push(candidate.len());
    if boundaries.len() <= query_tokens.len() {
        return None;
    }

    partitioned_token_distance_impl(query_tokens, candidate, &boundaries, 0, 0, limit)
}

fn partitioned_token_distance_impl(
    query_tokens: &[&str],
    candidate: &str,
    boundaries: &[usize],
    token_idx: usize,
    start_boundary: usize,
    remaining: usize,
) -> Option<(usize, usize, usize)> {
    let last_token = token_idx + 1 == query_tokens.len();
    let min_end_boundary = start_boundary + 1;
    let max_end_boundary = boundaries.len() - (query_tokens.len() - token_idx);
    let mut best = None;

    for end_boundary in min_end_boundary..=max_end_boundary {
        if !last_token && end_boundary == boundaries.len() - 1 {
            break;
        }

        let end_boundary = if last_token { boundaries.len() - 1 } else { end_boundary };
        let segment = &candidate[boundaries[start_boundary]..boundaries[end_boundary]];
        let (cost, structure, position_rank, _) =
            match best_token_match(query_tokens[token_idx], segment, remaining) {
                Some(values) => values,
                None => continue,
            };
        if cost > remaining {
            continue;
        }

        let total = if last_token {
            (cost, structure, position_rank)
        } else {
            let Some((tail_cost, tail_structure, tail_position)) = partitioned_token_distance_impl(
                query_tokens,
                candidate,
                boundaries,
                token_idx + 1,
                end_boundary,
                remaining - cost,
            ) else {
                continue;
            };
            (cost + tail_cost, structure + tail_structure, position_rank + tail_position)
        };

        best = Some(match best {
            None => total,
            Some(current) if total < current => total,
            Some(current) => current,
        });
        if best == Some((0, 0, 0)) {
            break;
        }

        if last_token {
            break;
        }
    }

    best
}

#[derive(Clone, Copy)]
struct TokenCandidate<'a> {
    token: &'a str,
    path_position: usize,
}

fn aligned_token_distance(
    query_tokens: &[&str],
    candidate_tokens: &[TokenCandidate<'_>],
    limit: usize,
) -> Option<(usize, usize, usize)> {
    if candidate_tokens.len() < query_tokens.len() {
        return None;
    }

    let mut best = None;
    for start in 0..=candidate_tokens.len() - query_tokens.len() {
        let mut distance = 0;
        let mut path_position = 0;
        let mut structure = 0;
        let mut failed = false;
        for (query_token, candidate) in
            query_tokens.iter().zip(candidate_tokens[start..start + query_tokens.len()].iter())
        {
            let Some(remaining) = limit.checked_sub(distance) else {
                failed = true;
                break;
            };
            let Some((cost, penalty, position_rank, _)) =
                best_token_match(query_token, candidate.token, remaining)
            else {
                failed = true;
                break;
            };
            distance += cost;
            path_position += candidate.path_position * 3 + position_rank;
            structure += penalty;
            if distance > limit {
                failed = true;
                break;
            }
        }

        if failed {
            continue;
        }

        let total = (distance, path_position, structure);
        best = Some(match best {
            None => total,
            Some(current) if total < current => total,
            Some(current) => current,
        });
        if best == Some((0, 0, 0)) {
            break;
        }
    }

    best
}

fn best_token_match(
    query: &str,
    candidate: &str,
    limit: usize,
) -> Option<(usize, usize, usize, usize)> {
    if query.is_empty() || candidate.is_empty() {
        return None;
    }

    let candidate_len = candidate.chars().count();
    let query_len = query.chars().count();
    let mut boundaries: Vec<usize> = candidate.char_indices().map(|(idx, _)| idx).collect();
    boundaries.push(candidate.len());

    let mut best = None;
    for start in 0..candidate_len {
        let min_len = 1usize.max(query_len.saturating_sub(limit));
        let max_len = (query_len + limit).min(candidate_len - start);
        for len in min_len..=max_len {
            let end = start + len;
            let segment = &candidate[boundaries[start]..boundaries[end]];
            let Some(distance) = bounded_damerau_levenshtein(query, segment, limit) else {
                continue;
            };
            let penalty = start + (candidate_len - end);
            let position_rank = if start == 0 {
                0
            } else if end == candidate_len {
                1
            } else {
                2
            };
            let total = (distance, penalty, position_rank, len);
            best = Some(match best {
                None => total,
                Some(current) if total < current => total,
                Some(current) => current,
            });
            if best == Some((0, 0, 0, candidate_len)) {
                return best;
            }
        }
    }

    best
}

fn path_components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|component| !component.is_empty()).collect()
}

fn split_tokens(component: &str) -> impl Iterator<Item = &str> {
    component
        .split(|c: char| matches!(c, '/' | '\\' | '-' | '_' | '.') || c.is_whitespace())
        .filter(|token| !token.is_empty())
}

fn bounded_damerau_levenshtein(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left: Vec<_> = left.chars().collect();
    let right: Vec<_> = right.chars().collect();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }

    let inf = limit + 1;
    let mut prev_prev = vec![inf; right.len() + 1];
    let mut prev: Vec<_> = (0..=right.len()).collect();
    let mut curr = vec![inf; right.len() + 1];

    for i in 1..=left.len() {
        curr.fill(inf);
        curr[0] = i;

        let start = i.saturating_sub(limit).max(1);
        let end = (i + limit).min(right.len());
        if start > end {
            return None;
        }

        let mut row_min = inf;
        for j in start..=end {
            let cost = usize::from(left[i - 1] != right[j - 1]);
            let deletion = prev[j] + 1;
            let insertion = curr[j - 1] + 1;
            let substitution = prev[j - 1] + cost;
            let mut cell = deletion.min(insertion).min(substitution);

            if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                cell = cell.min(prev_prev[j - 2] + 1);
            }

            curr[j] = cell;
            row_min = row_min.min(cell);
        }

        if row_min > limit {
            return None;
        }

        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }

    let distance = prev[right.len()];
    (distance <= limit).then_some(distance)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn dir(path: &str, rank: Rank, last_accessed: Epoch) -> Dir<'static> {
        Dir { path: Cow::Owned(path.to_string()), rank, last_accessed }
    }

    #[test]
    fn typo_matches_basename() {
        let dir = dir("/home/lewis/xfce4-terminal", 10.0, 100);
        let candidate = best_match(&dir, "xfce4-terinal", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn basename_token_prefixes_do_not_count_as_typos() {
        let dir = dir("/home/lewis/xfce4-terminal", 10.0, 100);
        let candidate = best_match(&dir, "xzfce-ter", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn spaced_query_tokens_can_match_single_compound_token() {
        let dir = dir("/home/lewis/applicationlauncher", 10.0, 100);
        let candidate = best_match(&dir, "app laucnh", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.path_position, 0);
        assert_eq!(candidate.distance, 1);
        assert!(candidate.structure > 0);
    }

    #[test]
    fn spaced_query_tokens_can_match_component_sequence() {
        let dir = dir("/home/lewis/tasks/config", 10.0, 100);
        let candidate = best_match(&dir, "tasks cinfig", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.path_position, 3);
        assert_eq!(candidate.distance, 1);
        assert_eq!(candidate.structure, 0);
    }

    #[test]
    fn structure_penalty_tracks_token_positions() {
        assert_eq!(best_token_match("cinfig", "redragonmouseconfig", 3), Some((1, 13, 1, 6)));
    }

    #[test]
    fn component_sequence_can_match_substring_inside_token() {
        let dir = dir("/home/lewis/tasks/redragonmouseconfig", 10.0, 100);
        let candidate = best_match(&dir, "tasks cinfig", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.path_position, 4);
        assert_eq!(candidate.distance, 1);
        assert_eq!(candidate.structure, 13);
    }

    #[test]
    fn multiple_tokens_can_match_the_same_non_basename_component() {
        let dir = dir("/home/lewis/Dev/applicationlauncher/target/release", 10.0, 100);
        let candidate = best_match(&dir, "ap laun", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::OtherComponent);
        assert_eq!(candidate.distance, 0);
        assert_eq!(candidate.path_position, 12);
    }

    #[test]
    fn long_queries_use_half_length_typo_limit() {
        let dir = dir("/home/lewis/xfce4-terminal", 10.0, 100);
        let candidate = best_match(&dir, "xgce4-tremriianl", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::Basename);
        assert_eq!(candidate.distance, 5);
    }

    #[test]
    fn typo_matches_component_token() {
        let dir = dir("/home/lewis/xfce4-terminal", 10.0, 100);
        let candidate = best_match(&dir, "x4ce4", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::BasenameToken);
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn five_character_queries_allow_half_length_typos() {
        let dir = dir("/home/lewis/xfce4-terminal", 10.0, 100);
        let candidate = best_match(&dir, "zgce4", 100).unwrap();
        assert_eq!(candidate.scope, MatchScope::BasenameToken);
        assert_eq!(candidate.distance, 2);
        assert_eq!(candidate.ratio, 0.4);
    }

    #[test]
    fn single_character_queries_are_not_corrected() {
        let dir = dir("/home/lewis/foobar", 10.0, 100);
        assert!(best_match(&dir, "f", 100).is_none());
    }

    #[test]
    fn short_queries_allow_one_typo() {
        let dir = dir("/home/lewis/foo", 10.0, 100);
        let candidate = best_match(&dir, "foa", 100).unwrap();
        assert_eq!(candidate.distance, 1);
    }

    #[test]
    fn ambiguous_equal_distance_matches_are_rejected() {
        let dir1 = dir("/home/lewis/terminal", 10.0, 100);
        let dir2 = dir("/home/lewis/terminap", 10.0, 100);
        let mut matches = vec![
            best_match(&dir1, "terminak", 100).unwrap(),
            best_match(&dir2, "terminak", 100).unwrap(),
        ];
        sort_matches(&mut matches);
        assert!(is_ambiguous(&matches[0], &matches[1]));
    }

    #[test]
    fn frecency_resolves_otherwise_ambiguous_matches() {
        let preferred = dir("/home/lewis/repos/xfce4-terminal", 20.0, 100);
        let other = dir("/home/lewis/Dev/config/xfce4-terminal", 10.0, 100);
        let mut matches = vec![
            best_match(&preferred, "xgce4-tremriianl", 100).unwrap(),
            best_match(&other, "xgce4-tremriianl", 100).unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].dir.path, "/home/lewis/repos/xfce4-terminal");
        assert!(!is_ambiguous(&matches[0], &matches[1]));
    }

    #[test]
    fn basename_match_beats_parent_component_match() {
        let parent = dir("/home/xfce4/project", 100.0, 100);
        let basename = dir("/home/lewis/xfce4-terminal", 1.0, 100);
        let mut matches = vec![
            best_match(&parent, "x4ce4", 100).unwrap(),
            best_match(&basename, "x4ce4", 100).unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].dir.path, "/home/lewis/xfce4-terminal");
        assert_eq!(matches[0].scope, MatchScope::BasenameToken);
    }

    #[test]
    fn lower_edit_distance_beats_frecency() {
        let closer = dir("/home/lewis/xfce4-terminal", 1.0, 100);
        let farther = dir("/home/lewis/xfce4-terminals", 1000.0, 100);
        let mut matches = vec![
            best_match(&closer, "xfce4-terinal", 100).unwrap(),
            best_match(&farther, "xfce4-terinal", 100).unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].dir.path, "/home/lewis/xfce4-terminal");
    }

    #[test]
    fn frecency_breaks_ties_after_distance_ratio_and_scope() {
        let low_score = dir("/tmp/xfce4-terminal", 1.0, 100);
        let high_score = dir("/var/xfce4-utility", 100.0, 100);
        let mut matches = vec![
            best_match(&low_score, "x4ce4", 100).unwrap(),
            best_match(&high_score, "x4ce4", 100).unwrap(),
        ];
        sort_matches(&mut matches);
        assert_eq!(matches[0].dir.path, "/var/xfce4-utility");
        assert_eq!(matches[0].distance, matches[1].distance);
        assert_eq!(matches[0].scope, matches[1].scope);
        assert_eq!(matches[0].ratio, matches[1].ratio);
    }
}

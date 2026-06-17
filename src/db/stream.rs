use std::iter::Rev;
use std::ops::Range;
use std::path::Path;
use std::{fs, path};

use glob::Pattern;

use crate::db::typo::{self, TypoMatch};
use crate::db::{Database, Dir, Epoch};
use crate::util::{self, MONTH};

pub struct Stream<'a> {
    db: &'a mut Database,
    idxs: Rev<Range<usize>>,
    options: StreamOptions,
}

impl<'a> Stream<'a> {
    pub fn new(db: &'a mut Database, options: StreamOptions) -> Self {
        db.sort_for_query(options.now, &options.keywords);
        let idxs = (0..db.dirs().len()).rev();
        Stream { db, idxs, options }
    }

    pub fn next(&mut self) -> Option<&Dir<'_>> {
        while let Some(idx) = self.idxs.next() {
            let dir = &self.db.dirs()[idx];

            if !self.filter_by_keywords(&dir.path) {
                continue;
            }

            if !self.filter_by_base_dir(&dir.path) {
                continue;
            }

            if !self.filter_by_exclude(&dir.path) {
                self.db.swap_remove(idx);
                continue;
            }

            // Exists queries are slow, this should always be checked last.
            if !self.filter_by_exists(&dir.path) {
                if dir.last_accessed < self.options.ttl {
                    self.db.swap_remove(idx);
                }
                continue;
            }

            let dir = &self.db.dirs()[idx];
            return Some(dir);
        }

        None
    }

    pub fn typo_matches(
        &mut self,
        keywords: &[String],
        exclude_path: Option<&str>,
        now: Epoch,
    ) -> Vec<TypoMatch<'_>> {
        let Some(query) = typo_query(keywords) else {
            return Vec::new();
        };

        let mut matches = Vec::new();
        for dir in self.db.dirs() {
            if Some(dir.path.as_ref()) == exclude_path {
                continue;
            }
            if !self.filter_by_base_dir(&dir.path)
                || !self.filter_by_exclude(&dir.path)
                || !self.filter_by_exists(&dir.path)
            {
                continue;
            }
            if let Some(candidate) = typo::best_match(dir, &query, now) {
                matches.push(candidate);
            }
        }

        typo::sort_matches(&mut matches);
        matches
    }

    fn filter_by_base_dir(&self, path: &str) -> bool {
        match &self.options.base_dir {
            Some(base_dir) => Path::new(path).starts_with(base_dir),
            None => true,
        }
    }

    fn filter_by_exclude(&self, path: &str) -> bool {
        !self.options.exclude.iter().any(|pattern| pattern.matches(path))
    }

    fn filter_by_exists(&self, path: &str) -> bool {
        if !self.options.exists {
            return true;
        }

        // The logic here is reversed - if we resolve symlinks when adding entries to
        // the database, we should not return symlinks when querying from
        // the database.
        let resolver =
            if self.options.resolve_symlinks { fs::symlink_metadata } else { fs::metadata };
        resolver(path).map(|metadata| metadata.is_dir()).unwrap_or_default()
    }

    fn filter_by_keywords(&self, path: &str) -> bool {
        match_qualities(path, &self.options.keywords).is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum MatchQuality {
    Contains = 0,
    Suffix = 1,
    Prefix = 2,
    Exact = 3,
}

pub(crate) fn match_penalty(path: &str, keywords: &[String]) -> Option<usize> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(0),
    };

    let path = util::to_lowercase(path);
    let mut path = path.as_str();
    let mut penalty = 0;

    let idx = path.rfind(keywords_last)?;
    if path[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    penalty += match_position_penalty(path, idx, keywords_last.len());
    path = &path[..idx];

    for keyword in keywords.iter().rev() {
        let idx = path.rfind(keyword)?;
        penalty += match_position_penalty(path, idx, keyword.len());
        path = &path[..idx];
    }

    Some(penalty)
}

pub(crate) fn match_path_position(path: &str, keywords: &[String]) -> Option<usize> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(0),
    };

    let full_path = util::to_lowercase(path);
    let mut search = full_path.as_str();
    let mut position = 0;

    let idx = search.rfind(keywords_last)?;
    if search[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    position += path_position_class(&full_path, idx, keywords_last.len());
    search = &search[..idx];

    for keyword in keywords.iter().rev() {
        let idx = search.rfind(keyword)?;
        position += path_position_class(&full_path, idx, keyword.len());
        search = &search[..idx];
    }

    Some(position)
}

pub(crate) fn match_qualities(path: &str, keywords: &[String]) -> Option<Vec<MatchQuality>> {
    let (keywords_last, keywords) = match keywords.split_last() {
        Some(split) => split,
        None => return Some(Vec::new()),
    };

    let path = util::to_lowercase(path);
    let mut path = path.as_str();
    let mut qualities = Vec::with_capacity(keywords.len() + 1);

    let idx = path.rfind(keywords_last)?;
    if path[idx + keywords_last.len()..].contains(path::is_separator) {
        return None;
    }
    qualities.push(match_quality(path, idx, keywords_last.len()));
    path = &path[..idx];

    for keyword in keywords.iter().rev() {
        let idx = path.rfind(keyword)?;
        qualities.push(match_quality(path, idx, keyword.len()));
        path = &path[..idx];
    }

    Some(qualities)
}

fn match_quality(path: &str, idx: usize, len: usize) -> MatchQuality {
    let (_, quality) = token_match_details(path, idx, len);
    quality
}

fn match_position_penalty(path: &str, idx: usize, len: usize) -> usize {
    let (token_len, _) = token_match_details(path, idx, len);
    token_len.saturating_sub(len)
}

fn component_distance_from_basename(path: &str, idx: usize, len: usize) -> usize {
    let component_end =
        path[idx + len..].find(path::is_separator).map_or(path.len(), |pos| idx + len + pos);
    path[component_end..].chars().filter(|&c| path::is_separator(c)).count()
}

fn path_position_class(path: &str, idx: usize, len: usize) -> usize {
    let component_distance = component_distance_from_basename(path, idx, len);
    let (_, quality) = token_match_details(path, idx, len);
    component_distance * 3 + position_rank(quality)
}

fn position_rank(quality: MatchQuality) -> usize {
    match quality {
        MatchQuality::Exact | MatchQuality::Prefix => 0,
        MatchQuality::Suffix => 1,
        MatchQuality::Contains => 2,
    }
}

fn token_match_details(path: &str, idx: usize, len: usize) -> (usize, MatchQuality) {
    let component_start = path[..idx].rfind(path::is_separator).map_or(0, |pos| pos + 1);
    let component_end =
        path[idx + len..].find(path::is_separator).map_or(path.len(), |pos| idx + len + pos);
    let component = &path[component_start..component_end];
    let local_start = idx - component_start;
    let local_end = local_start + len;

    let mut token_start = 0;
    for (token_idx, token) in
        component.split(|c: char| matches!(c, '-' | '_' | '.') || c.is_whitespace()).enumerate()
    {
        if token.is_empty() {
            continue;
        }
        let search_start = if token_idx == 0 { token_start } else { token_start + 1 };
        let relative = component[search_start..].find(token).unwrap();
        token_start = search_start + relative;
        let token_end = token_start + token.len();
        if token_start <= local_start && local_end <= token_end {
            return (
                token.len(),
                if token.len() == len {
                    MatchQuality::Exact
                } else if local_start == token_start {
                    MatchQuality::Prefix
                } else if local_end == token_end {
                    MatchQuality::Suffix
                } else {
                    MatchQuality::Contains
                },
            );
        }
        token_start = token_end;
    }

    (len, MatchQuality::Contains)
}

fn typo_query(keywords: &[String]) -> Option<String> {
    if keywords.is_empty() || keywords.iter().any(String::is_empty) {
        return None;
    }

    Some(keywords.iter().map(util::to_lowercase).collect::<Vec<_>>().join(" "))
}

pub struct StreamOptions {
    /// The current time.
    now: Epoch,

    /// Only directories matching these keywords will be returned.
    keywords: Vec<String>,

    /// Directories that match any of these globs will be lazily removed.
    exclude: Vec<Pattern>,

    /// Directories will only be returned if they exist on the filesystem.
    exists: bool,

    /// Whether to resolve symlinks when checking if a directory exists.
    resolve_symlinks: bool,

    /// Directories that do not exist and haven't been accessed since TTL will
    /// be lazily removed.
    ttl: Epoch,

    /// Only return directories within this parent directory
    /// Does not check if the path exists
    base_dir: Option<String>,
}

impl StreamOptions {
    pub fn new(now: Epoch) -> Self {
        StreamOptions {
            now,
            keywords: Vec::new(),
            exclude: Vec::new(),
            exists: false,
            resolve_symlinks: false,
            ttl: now.saturating_sub(3 * MONTH),
            base_dir: None,
        }
    }

    pub fn with_keywords<I>(mut self, keywords: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.keywords = keywords.into_iter().map(util::to_lowercase).collect();
        self
    }

    pub fn with_exclude(mut self, exclude: Vec<Pattern>) -> Self {
        self.exclude = exclude;
        self
    }

    pub fn with_exists(mut self, exists: bool) -> Self {
        self.exists = exists;
        self
    }

    pub fn with_resolve_symlinks(mut self, resolve_symlinks: bool) -> Self {
        self.resolve_symlinks = resolve_symlinks;
        self
    }

    pub fn with_base_dir(mut self, base_dir: Option<String>) -> Self {
        self.base_dir = base_dir;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;

    use super::*;

    #[rstest]
    // Case normalization
    #[case(&["fOo", "bAr"], "/foo/bar", true)]
    // Last component
    #[case(&["ba"], "/foo/bar", true)]
    #[case(&["fo"], "/foo/bar", false)]
    // Slash as suffix
    #[case(&["foo/"], "/foo", false)]
    #[case(&["foo/"], "/foo/bar", true)]
    #[case(&["foo/"], "/foo/bar/baz", false)]
    #[case(&["foo", "/"], "/foo", false)]
    #[case(&["foo", "/"], "/foo/bar", true)]
    #[case(&["foo", "/"], "/foo/bar/baz", true)]
    // Split components
    #[case(&["/", "fo", "/", "ar"], "/foo/bar", true)]
    #[case(&["oo/ba"], "/foo/bar", true)]
    // Overlap
    #[case(&["foo", "o", "bar"], "/foo/bar", false)]
    #[case(&["/foo/", "/bar"], "/foo/bar", false)]
    #[case(&["/foo/", "/bar"], "/foo/baz/bar", true)]
    fn query(#[case] keywords: &[&str], #[case] path: &str, #[case] is_match: bool) {
        let db = &mut Database::new(PathBuf::new(), Vec::new(), |_| Vec::new(), false);
        let options = StreamOptions::new(0).with_keywords(keywords.iter());
        let stream = Stream::new(db, options);
        assert_eq!(is_match, stream.filter_by_keywords(path));
    }

    #[test]
    fn match_penalty_sums_token_positions() {
        let keywords = ["asks", "onfig"];
        let keywords = keywords.into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(match_penalty("/home/lewis/tasks/config", &keywords), Some(2));
    }

    #[test]
    fn match_path_position_prefers_basename_over_parent_components() {
        let keywords = ["ap", "laun"];
        let keywords = keywords.into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(match_path_position("/home/lewis/Dev/applicationlauncher", &keywords), Some(2));
        assert_eq!(
            match_path_position("/home/lewis/Dev/applicationlauncher/target/release", &keywords),
            None
        );
    }
}

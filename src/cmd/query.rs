use std::collections::HashSet;
use std::io::{self, Write};

use anyhow::{Context, Result, bail};

use crate::cmd::{Query, Run};
use crate::config;
use crate::db::{
    Database, Epoch, Stream, StreamOptions, TypoMatch, match_path_position, match_penalty,
};
use crate::error::BrokenPipeHandler;
use crate::util::{self, Fzf, FzfChild, format_path_position};

impl Run for Query {
    fn run(&self) -> Result<()> {
        let mut db = crate::db::Database::open()?;
        self.query(&mut db).and(db.save())
    }
}

impl Query {
    fn query(&self, db: &mut Database) -> Result<()> {
        let now = util::current_time()?;
        let mut stream = self.get_stream(db, now)?;
        let typo_fallback = self.typo_fallback || config::typo_fallback();

        if self.interactive {
            self.query_interactive(&mut stream, now, typo_fallback)
        } else if self.list {
            self.query_list(&mut stream, now, typo_fallback)
        } else {
            self.query_first(&mut stream, now, typo_fallback)
        }
    }

    fn query_interactive(
        &self,
        stream: &mut Stream,
        now: Epoch,
        typo_fallback: bool,
    ) -> Result<()> {
        let mut fzf = Self::get_fzf()?;
        let mut wrote_any = false;
        let mut seen_paths = HashSet::new();
        let selection = loop {
            match stream.next() {
                Some(dir) if Some(dir.path.as_ref()) == self.exclude.as_deref() => continue,
                Some(dir) => {
                    wrote_any = true;
                    seen_paths.insert(dir.path.as_ref().to_owned());
                    let path_position = match_path_position(&dir.path, &self.keywords).unwrap_or(0);
                    let structure = match_penalty(&dir.path, &self.keywords).unwrap_or(0);
                    if let Some(selection) =
                        fzf.write_query(dir, now, 0, path_position, structure)?
                    {
                        break selection;
                    }
                }
                None if typo_fallback => {
                    let mut selection = None;
                    for candidate in
                        stream.typo_matches(&self.keywords, self.exclude.as_deref(), now)
                    {
                        if seen_paths.contains(candidate.dir.path.as_ref()) {
                            continue;
                        }
                        if let Some(selected) = fzf.write_typo(&candidate, now)? {
                            selection = Some(selected);
                            break;
                        }
                    }
                    break match selection {
                        Some(selection) => selection,
                        None => fzf.wait()?,
                    };
                }
                None if wrote_any => break fzf.wait()?,
                None => break fzf.wait()?,
            }
        };

        if self.score {
            print!("{selection}");
        } else {
            let (_, path) =
                selection.split_once('\t').context("could not read selection from fzf")?;
            print!("{path}");
        }
        Ok(())
    }

    fn query_list(&self, stream: &mut Stream, now: Epoch, typo_fallback: bool) -> Result<()> {
        let handle = &mut io::stdout().lock();
        let mut wrote_any = false;
        while let Some(dir) = stream.next() {
            if Some(dir.path.as_ref()) == self.exclude.as_deref() {
                continue;
            }
            let dir = if self.score { dir.display().with_score(now) } else { dir.display() };
            writeln!(handle, "{dir}").pipe_exit("stdout")?;
            wrote_any = true;
        }

        if !wrote_any && typo_fallback {
            for candidate in stream.typo_matches(&self.keywords, self.exclude.as_deref(), now) {
                let dir = if self.score {
                    candidate.dir.display().with_score(now)
                } else {
                    candidate.dir.display()
                };
                writeln!(handle, "{dir}").pipe_exit("stdout")?;
            }
        }
        Ok(())
    }

    fn query_first(&self, stream: &mut Stream, now: Epoch, typo_fallback: bool) -> Result<()> {
        let handle = &mut io::stdout();
        let mut excluded_only = false;
        while let Some(dir) = stream.next() {
            if Some(dir.path.as_ref()) == self.exclude.as_deref() {
                excluded_only = true;
                continue;
            }
            let dir = if self.score { dir.display().with_score(now) } else { dir.display() };
            return writeln!(handle, "{dir}").pipe_exit("stdout");
        }

        if typo_fallback {
            let matches = stream.typo_matches(&self.keywords, self.exclude.as_deref(), now);
            match matches.as_slice() {
                [] => {}
                [candidate, ..]
                    if matches.get(1).is_some_and(|next| is_ambiguous(candidate, next)) =>
                {
                    bail!("{}", format_ambiguous_matches(&matches));
                }
                [candidate, ..] => {
                    let dir = if self.score {
                        candidate.dir.display().with_score(now)
                    } else {
                        candidate.dir.display()
                    };
                    return writeln!(handle, "{dir}").pipe_exit("stdout");
                }
            }
        }

        if excluded_only {
            bail!("you are already in the only match")
        } else {
            bail!("no match found")
        }
    }

    fn get_stream<'a>(&self, db: &'a mut Database, now: Epoch) -> Result<Stream<'a>> {
        let mut options = StreamOptions::new(now)
            .with_keywords(self.keywords.iter().map(|s| s.as_str()))
            .with_exclude(config::exclude_dirs()?)
            .with_base_dir(self.base_dir.clone());
        if !self.all {
            let resolve_symlinks = config::resolve_symlinks();
            options = options.with_exists(true).with_resolve_symlinks(resolve_symlinks);
        }

        let stream = Stream::new(db, options);
        Ok(stream)
    }

    fn get_fzf() -> Result<FzfChild> {
        let mut fzf = Fzf::new()?;
        if let Some(fzf_opts) = config::fzf_opts() {
            fzf.env("FZF_DEFAULT_OPTS", fzf_opts)
        } else {
            fzf.args([
                // Search mode
                "--exact",
                // Search result
                "--no-sort",
                // Interface
                "--bind=ctrl-z:ignore,btab:up,tab:down",
                "--cycle",
                "--keep-right",
                "--no-mouse",
                // Layout
                "--border=sharp", // rounded edges don't display correctly on some terminals
                "--height=45%",
                "--info=inline",
                "--layout=reverse",
                // Display
                "--tabstop=1",
                // Scripting
                "--exit-0",
            ])
            .enable_preview()
        }
        .spawn()
    }
}

fn is_ambiguous(first: &TypoMatch<'_>, second: &TypoMatch<'_>) -> bool {
    crate::db::typo::is_ambiguous(first, second)
}

fn format_ambiguous_matches(matches: &[TypoMatch<'_>]) -> String {
    let mut message = String::from("ambiguous typo match");
    for candidate in matches.iter().take(5) {
        message.push_str(&format!(
            "\n  d={} p={} ratio={:.3} scope={} {}",
            candidate.distance,
            format_path_position(candidate.path_position, candidate.structure),
            candidate.ratio,
            candidate.scope,
            candidate.dir.path
        ));
    }
    message
}

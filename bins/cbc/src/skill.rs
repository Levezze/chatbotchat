//! `cbc install-skill` — write the bundled CBC skills into every AI coding harness on the
//! machine, so CBC delivers its own agent guidance with no external devkit checkout.
//!
//! CBC ships a small family of skills: `cbc` (drive a room well) plus the orchestration
//! set `cbc-orchestrator` / `cbc-worker` / `cbc-peer` / `cbc-recap` / `cbc-reconcile`
//! (coordinate many agents across one or more repos), `cbc-refresh` (replace a polluted room
//! with a fresh one while preserving the thread), `cbc-checkup` (orchestrator heartbeat sweep
//! with escalating backoff), and `cbc-clean` (consensus-close a line and optionally remove the
//! worktree). Each skill's text is embedded
//! ([`BUNDLED_SKILLS`] via
//! `include_str!`) so
//! `cargo install` — which ships only the compiled binary — carries them, and the installed
//! skills always match this binary's behavior.
//!
//! Two harnesses, two layouts ([`Harness`]). Claude Code discovers a user-scope skill from
//! `~/.claude/skills/<name>/SKILL.md` alone. Codex reads the same `SKILL.md` from
//! `~/.codex/skills/<name>/`, but only surfaces it as a `/<name>` command when the skill also
//! carries `agents/openai.yaml`; without that file the skill dir is inert. Those two paths are
//! only the DEFAULTS — both harnesses relocate their configuration home on an env var
//! (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) and then read skills from the moved home, so
//! [`Harness::home`] follows the var. Installing into a hardcoded `~/.claude` under a
//! relocated home writes a directory nobody reads, and the skills silently never appear.
//!
//! The yaml is *derived* from the skill's own frontmatter ([`codex_metadata`]) rather than
//! checked in beside it, so a skill and its Codex metadata cannot drift apart. Deriving it
//! means meeting Codex's contract for the file (its own `skill-creator` reference states it):
//! `interface.short_description` must be 25-64 characters — a CBC `description:` is written
//! for a model choosing whether to load the skill and runs far longer, so it is trimmed to a
//! clause ([`short_description`]) — and `interface.default_prompt` must invoke the skill
//! explicitly as `$name`.
//!
//! Layering mirrors `settings.rs`: a pure classifier ([`classify`]) decides what a target
//! currently is from an injected dir, and the read/back-up/write glue ([`install_one`]) is
//! path-injected so every branch is tested against a tempdir. [`install_all`] just loops the
//! family. The sharp edge it guards: devkit symlinks `<skills>/<name>` → its own
//! source, so writing through that symlink would corrupt devkit's file. [`classify`] detects
//! a symlink via `symlink_metadata` — at the skill dir itself, at any file the harness owns,
//! or at a directory on the way to one — and [`install_one`] refuses to follow any of them
//! unless `force` (which removes the link itself, never its target).

use anyhow::Context;
use std::path::{Path, PathBuf};

/// One bundled skill: its directory name under a harness's skills dir and its `SKILL.md`
/// text, embedded at compile time so the binary is self-contained (`cargo install` carries
/// no repo files). chatbotchat is the canonical source of these copies; keep `cbc`'s body in
/// sync with the agent-facing prose in `mcp.rs`.
struct BundledSkill {
    name: &'static str,
    body: &'static str,
}

/// An AI coding harness CBC installs its skills into. Each has its own skills directory and
/// its own idea of what files a skill is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    /// Skills at `~/.claude/skills/<name>/SKILL.md`.
    ClaudeCode,
    /// Skills at `~/.codex/skills/<name>/`, with `agents/openai.yaml` alongside `SKILL.md`.
    Codex,
}

/// Every harness CBC knows how to install into, in the order a full install visits them.
pub const HARNESSES: &[Harness] = &[Harness::ClaudeCode, Harness::Codex];

impl Harness {
    /// How to name this harness in a message to the user.
    pub fn label(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "Claude Code",
            Harness::Codex => "Codex",
        }
    }

    /// The value this harness takes on `--harness`.
    pub fn flag(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "claude",
            Harness::Codex => "codex",
        }
    }

    /// The env var that relocates this harness's configuration home, and the default
    /// directory under `$HOME` when it is unset. Both harnesses let a user move their
    /// config elsewhere, and both then read skills from the moved home — so honouring
    /// these is what keeps an install from writing into a directory nobody reads.
    fn home_env(self) -> (&'static str, &'static str) {
        match self {
            Harness::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude"),
            Harness::Codex => ("CODEX_HOME", ".codex"),
        }
    }

    /// The harness's home directory. A harness whose home is absent is not installed on
    /// this machine, which is how an automatic install decides whom to write for.
    pub fn home(self) -> anyhow::Result<PathBuf> {
        let (var, default) = self.home_env();
        // An empty var is the shell's way of saying "unset"; treating it as a path would
        // resolve the skills dir to a bare relative `skills/` in the process's cwd.
        if let Some(configured) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(configured));
        }
        let home = std::env::var_os("HOME")
            .with_context(|| format!("HOME not set; cannot locate {}", self.label()))?;
        Ok(PathBuf::from(home).join(default))
    }

    /// The harness's *user* scope skills dir, which applies across every project. Each
    /// skill lands at `<this>/<name>/`.
    pub fn skills_dir(self) -> anyhow::Result<PathBuf> {
        Ok(self.home()?.join("skills"))
    }

    /// The files this harness wants for one skill, as (path relative to the skill dir,
    /// contents). Claude Code takes the `SKILL.md` alone; Codex also needs the
    /// `agents/openai.yaml` that makes the skill slash-invocable.
    fn files(self, skill: &BundledSkill) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let mut files = vec![(PathBuf::from("SKILL.md"), skill.body.to_string())];
        if self == Harness::Codex {
            files.push((
                PathBuf::from("agents").join("openai.yaml"),
                codex_metadata(skill.name, skill.body)?,
            ));
        }
        Ok(files)
    }
}

/// The skills CBC ships and auto-installs. Each lives at `bins/cbc/skill/<name>/SKILL.md`;
/// a harness discovers a user-scope skill purely by its `<skills_dir>/<name>/` path.
/// `cbc` is kept first so the test suite can target it as the canonical single-skill case.
const BUNDLED_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "cbc",
        body: include_str!("../skill/cbc/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-orchestrator",
        body: include_str!("../skill/cbc-orchestrator/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-worker",
        body: include_str!("../skill/cbc-worker/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-peer",
        body: include_str!("../skill/cbc-peer/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-recap",
        body: include_str!("../skill/cbc-recap/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-reconcile",
        body: include_str!("../skill/cbc-reconcile/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-refresh",
        body: include_str!("../skill/cbc-refresh/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-checkup",
        body: include_str!("../skill/cbc-checkup/SKILL.md"),
    },
    BundledSkill {
        name: "cbc-clean",
        body: include_str!("../skill/cbc-clean/SKILL.md"),
    },
];

/// Build the `agents/openai.yaml` Codex reads to surface a skill as `/name`. Codex needs
/// three `interface` fields; all three are DERIVED from the skill's own frontmatter, so a
/// skill and its Codex metadata cannot drift apart the way a checked-in copy would.
fn codex_metadata(name: &str, body: &str) -> anyhow::Result<String> {
    let description = frontmatter_field(body, "description")
        .with_context(|| format!("the {name} skill has no `description:` in its frontmatter"))?;
    let summary = first_sentence(&description);
    let prompt = format!("Use ${name} to {}", lower_initial(&summary));
    Ok(format!(
        "interface:\n  display_name: {}\n  short_description: {}\n  default_prompt: {}\n",
        yaml_quote(&display_name(name)),
        yaml_quote(&short_description(name, &summary, &description)),
        yaml_quote(&prompt),
    ))
}

/// Codex's bounds on `interface.short_description`, from its own `skill-creator`
/// reference (`25-64 chars`); its generator hard-errors outside them.
const MIN_SHORT: usize = 25;
const MAX_SHORT: usize = 64;

/// Clause boundaries. Cutting a long summary at one of these ends on a finished phrase
/// ("Coordinate with peer orchestrators as symmetric siblings") instead of mid-thought.
const CLAUSE_BREAKS: &[&str] = &[" — ", " (", ", ", "; ", ": "];

/// A UI blurb for Codex's skill picker, within its 25-64 character bounds. A CBC skill's
/// `description:` is written for a model deciding whether to load it — 65 to 294
/// characters here — so it cannot be used raw: Codex's own generator rejects anything
/// longer than 64. Take the summary trimmed to fit; if that is too thin to scan, widen
/// to the whole description before giving up on the skill's own words and falling back
/// to a name-derived blurb (the last rung of the ladder Codex's generator walks).
fn short_description(name: &str, summary: &str, description: &str) -> String {
    for source in [summary, description] {
        let source = source.trim().trim_end_matches('.').trim();
        let candidate = if char_len(source) <= MAX_SHORT {
            source.to_string()
        } else {
            clause_prefix(source).unwrap_or_else(|| word_prefix(source))
        };
        if char_len(&candidate) >= MIN_SHORT {
            return candidate;
        }
    }
    let blurb = format!("Help with {} tasks", display_name(name));
    if char_len(&blurb) < MIN_SHORT {
        format!("Help with {} tasks and workflows", display_name(name))
    } else {
        blurb
    }
}

/// The longest prefix of `summary` ending at a clause boundary that lands inside the
/// bounds, or `None` when no boundary does.
fn clause_prefix(summary: &str) -> Option<String> {
    let cut = CLAUSE_BREAKS
        .iter()
        .flat_map(|brk| summary.match_indices(brk).map(|(at, _)| at))
        .filter(|&at| char_len(&summary[..at]) <= MAX_SHORT)
        .max()?;
    let head = summary[..cut].trim_end_matches([',', ';', ':']).trim();
    (char_len(head) >= MIN_SHORT).then(|| head.to_string())
}

/// As many whole words of `summary` as fit. A mid-word cut ("orchestra") reads as a typo;
/// a whole-word cut reads as an abbreviation.
fn word_prefix(summary: &str) -> String {
    let mut out = String::new();
    for word in summary.split_whitespace() {
        let width = char_len(&out) + char_len(word) + usize::from(!out.is_empty());
        if width > MAX_SHORT {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let out = out.trim_end_matches(['-', ',', ';', ':', '—']).trim();
    trim_dangling(out)
}

/// Words that only make sense pointing at what follows them. Truncation routinely strands
/// one ("working the same repo at"), which reads as a bug rather than a summary.
const CONNECTIVES: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "at", "in", "on", "of", "to", "for", "with", "under",
    "from", "by", "as", "into", "over", "per", "via", "that", "when", "while", "so", "than",
];

/// Drop a stranded connective, and the phrase it opens, off the end of a truncated blurb.
/// One pass over the last two words only: anything further back belongs to a phrase that
/// survived the cut intact, and repeating the pass eats those ("in a repo" -> "a" is a
/// connective -> gone). Never trims below Codex's floor; a blurb that short is worse than
/// a clipped one.
fn trim_dangling(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 2 {
        return text.to_string();
    }
    let tail = words.len() - 2;
    let Some(cut) = words[tail..]
        .iter()
        .position(|word| CONNECTIVES.contains(&word.to_lowercase().as_str()))
        .map(|offset| tail + offset)
    else {
        return text.to_string();
    };
    let trimmed = words[..cut].join(" ");
    if char_len(&trimmed) < MIN_SHORT {
        return text.to_string();
    }
    trimmed
}

/// Character count, not byte count: skill descriptions carry em-dashes, and Codex counts
/// its 25-64 bound in characters.
fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// Lowercase a summary's first letter so it reads as a clause after "Use $name to ".
/// Skill descriptions open with an imperative ("Run a sweep…" -> "run a sweep…"). An
/// initialism is left alone — "CBC" must not become "cBC" — which a lowercase second
/// character identifies.
fn lower_initial(summary: &str) -> String {
    let mut chars = summary.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest = chars.as_str();
    if rest.starts_with(|c: char| c.is_lowercase()) {
        first.to_lowercase().collect::<String>() + rest
    } else {
        summary.to_string()
    }
}

/// Read a single-line `<key>: <value>` field out of a SKILL.md YAML frontmatter block.
fn frontmatter_field(body: &str, key: &str) -> Option<String> {
    let rest = body.strip_prefix("---\n")?;
    let block = rest.split_once("\n---")?.0;
    let prefix = format!("{key}:");
    block
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .map(|v| v.trim().to_string())
}

/// Abbreviations whose trailing period does NOT end a sentence. Skill descriptions are
/// prose, so this list is about what shows up in prose, not exhaustive English.
const ABBREVIATIONS: &[&str] = &["e.g", "i.e", "etc", "vs", "cf", "Dr", "Mr", "Ms", "St"];

/// The first sentence of a description, always terminated with a period. Skill
/// descriptions open with a one-line summary and then elaborate; Codex shows only the
/// summary, so carrying the whole paragraph would flood its skill picker. A period that
/// merely closes an abbreviation is not a sentence end — splitting there would ship a
/// fragment like "Use when a room stalls (e.g." as the skill's whole summary.
fn first_sentence(description: &str) -> String {
    let mut searched = 0;
    while let Some(offset) = description[searched..].find(". ") {
        let end = searched + offset;
        let head = &description[..end];
        if !ABBREVIATIONS.iter().any(|abbrev| head.ends_with(abbrev)) {
            return format!("{}.", head.trim());
        }
        searched = end + ". ".len();
    }
    format!("{}.", description.trim().trim_end_matches('.'))
}

/// `cbc-orchestrator` -> `CBC Orchestrator`. Hyphens become spaces and each word is
/// capitalized, except `cbc` — an initialism that reads as noise title-cased ("Cbc").
fn display_name(name: &str) -> String {
    name.split('-')
        .map(|word| {
            if word == "cbc" {
                "CBC".to_string()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote a value as a YAML double-quoted scalar. Descriptions carry em-dashes, backticks
/// and slashes, none of which need escaping — but a stray quote or backslash would break
/// the file, so both are escaped.
fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What [`install_one`] did, so the caller can print an honest one-liner.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No skill dir existed; it was created with the harness's bundled files.
    Created,
    /// An out-of-date real file was refreshed (each prior one backed up to `.bak`).
    Updated,
    /// Every file the harness wants already matched the bundled copy; nothing written.
    AlreadyPresent,
    /// A symlink (the skill dir, devkit-managed, or a file inside it) was found and
    /// `force` was not set, so it was left untouched. Carries the link target.
    SkippedSymlink(PathBuf),
    /// A symlink was found and `force` replaced it with a real bundled copy.
    ReplacedSymlink,
}

/// The current state of `<skills_dir>/<name>`, classified WITHOUT following a symlink.
enum TargetState {
    /// Nothing at the path.
    Absent,
    /// A symlink we must not write *through* — the skill dir itself (devkit-managed), a
    /// file the harness owns, or a directory on the way to one. `link` is the symlink's
    /// own path (what `force` removes); `target` is what it points at (for the message).
    Symlink { link: PathBuf, target: PathBuf },
    /// A real directory holding real files; they either all match the bundled copies or
    /// they do not (a missing file counts as "does not match" → a rewrite fills it in).
    RealDir { matches: bool },
}

/// Classify `<skill_dir>` against the harness's `files` without dereferencing a symlink
/// (the corruption guard). `RealDir { matches }` reports whether every file already
/// equals the bundled copy we'd write.
fn classify(skill_dir: &Path, files: &[(PathBuf, String)]) -> anyhow::Result<TargetState> {
    let meta = match std::fs::symlink_metadata(skill_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TargetState::Absent),
        Err(e) => {
            return Err(e).with_context(|| format!("inspecting {}", skill_dir.display()));
        }
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(skill_dir).unwrap_or_else(|_| PathBuf::from("?"));
        return Ok(TargetState::Symlink {
            link: skill_dir.to_path_buf(),
            target,
        });
    }
    // The skill dir is a real dir. Guard everything inside it the same way: a symlinked
    // `SKILL.md`, a symlinked `agents/openai.yaml`, or a symlinked `agents/` directory
    // would each be followed by a write and clobber whatever it points at.
    let mut matches = true;
    for (relative, body) in files {
        let mut path = skill_dir.to_path_buf();
        for component in relative.iter() {
            path.push(component);
            if let Ok(m) = std::fs::symlink_metadata(&path) {
                if m.file_type().is_symlink() {
                    let target = std::fs::read_link(&path).unwrap_or_else(|_| PathBuf::from("?"));
                    return Ok(TargetState::Symlink { link: path, target });
                }
            }
        }
        let current = std::fs::read(&path)
            .map(|bytes| bytes == body.as_bytes())
            .unwrap_or(false);
        if !current {
            matches = false;
        }
    }
    Ok(TargetState::RealDir { matches })
}

/// Install every bundled skill into `<skills_dir>/<name>/` for `harness`, returning each
/// skill's name and [`Outcome`] so the caller can print one honest line per skill. Stops at
/// the first hard error (a skills dir we can't read/write), letting callers degrade.
pub fn install_all(
    harness: Harness,
    skills_dir: &Path,
    force: bool,
) -> anyhow::Result<Vec<(&'static str, Outcome)>> {
    let mut outcomes = Vec::with_capacity(BUNDLED_SKILLS.len());
    for skill in BUNDLED_SKILLS {
        let outcome = install_one(harness, skills_dir, skill, force)
            .with_context(|| format!("installing the {} skill", skill.name))?;
        outcomes.push((skill.name, outcome));
    }
    Ok(outcomes)
}

/// Write one bundled skill into `<skills_dir>/<name>/` in `harness`'s layout. Idempotent,
/// backs up a stale file before overwriting, and never writes *through* a devkit symlink
/// unless `force` (which replaces the symlink itself, leaving its target untouched).
/// Errors bubble so callers can degrade rather than crash.
fn install_one(
    harness: Harness,
    skills_dir: &Path,
    skill: &BundledSkill,
    force: bool,
) -> anyhow::Result<Outcome> {
    let skill_dir = skills_dir.join(skill.name);
    let files = harness.files(skill)?;

    match classify(&skill_dir, &files)? {
        TargetState::Absent => {
            write_files(&skill_dir, &files)?;
            Ok(Outcome::Created)
        }
        TargetState::RealDir { matches: true } => Ok(Outcome::AlreadyPresent),
        TargetState::RealDir { matches: false } => {
            back_up_stale(&skill_dir, &files)?;
            write_files(&skill_dir, &files)?;
            Ok(Outcome::Updated)
        }
        TargetState::Symlink { link, target } => {
            if !force {
                return Ok(Outcome::SkippedSymlink(target));
            }
            // Remove the symlink ITSELF (not its target — `remove_file` on a symlink
            // never touches what it points at), then write real dirs + files. `link`
            // is the skill dir for a devkit symlink, or an inner path for an inner
            // symlink; removing either leaves a real dir to write into.
            //
            // A skill dir can hold MORE than one symlink (a linked `SKILL.md` beside a
            // linked `agents/`), and `classify` reports only the first it meets — so
            // drain them until none is left. Writing after removing just one would go
            // straight through the survivor and clobber its target, which is the exact
            // corruption `force` promises not to cause. Each pass removes one link, so
            // the loop is bounded by how many the dir has.
            let mut link = link;
            loop {
                std::fs::remove_file(&link)
                    .with_context(|| format!("removing the symlink at {}", link.display()))?;
                match classify(&skill_dir, &files)? {
                    TargetState::Symlink { link: next, .. } => link = next,
                    _ => break,
                }
            }
            // What the links hid can include a REAL stale file — a symlinked `agents/`
            // beside a hand-edited `SKILL.md`. Those get the same backup any other
            // stale file gets; skipping it would make `--force` the one path that
            // destroys a user's edits without a copy.
            back_up_stale(&skill_dir, &files)?;
            write_files(&skill_dir, &files)?;
            Ok(Outcome::ReplacedSymlink)
        }
    }
}

/// Copy each stale file aside to `<file>.bak` before it is overwritten. A file that is
/// absent or already current is left alone, so an install never churns a backup.
fn back_up_stale(skill_dir: &Path, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
    for (relative, body) in files {
        let path = skill_dir.join(relative);
        let Ok(existing) = std::fs::read(&path) else {
            continue; // nothing there yet — the write below simply creates it
        };
        if existing == body.as_bytes() {
            continue;
        }
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        // The backup path gets the same guard as the files themselves: a `.bak` that is
        // a symlink would send this write to whatever it points at — an unrelated user
        // file — bypassing every check `classify` just made. The link is ours to drop;
        // `remove_file` unlinks it and leaves its target alone.
        if let Ok(meta) = std::fs::symlink_metadata(&backup) {
            if meta.file_type().is_symlink() {
                std::fs::remove_file(&backup)
                    .with_context(|| format!("removing the symlink at {}", backup.display()))?;
            }
        }
        std::fs::write(&backup, &existing)
            .with_context(|| format!("backing up to {}", backup.display()))?;
    }
    Ok(())
}

/// Create the skill dir and any nested dirs (idempotent) and write every bundled file.
fn write_files(skill_dir: &Path, files: &[(PathBuf, String)]) -> anyhow::Result<()> {
    for (relative, body) in files {
        let path = skill_dir.join(relative);
        let parent = path.parent().unwrap_or(skill_dir).to_path_buf();
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Report what [`install_one`] did for one skill, naming the harness and path explicitly.
pub fn print_outcome(harness: Harness, skills_dir: &Path, name: &str, outcome: &Outcome) {
    let skill_dir = skills_dir.join(name);
    let file = skill_dir.join("SKILL.md");
    let who = harness.label();
    let restart = format!("Start a fresh {who} session to pick it up.");
    match outcome {
        Outcome::Created => {
            println!(
                "Installed the {name} skill for {who}:\n  {}",
                file.display()
            );
            println!("{restart}");
        }
        Outcome::Updated => {
            println!("Updated the {name} skill for {who}:\n  {}", file.display());
            println!("(backed up anything stale alongside it as .bak)");
            println!("{restart}");
        }
        Outcome::AlreadyPresent => {
            println!(
                "The {name} skill for {who} at {} is already up to date; nothing to do.",
                file.display()
            );
        }
        Outcome::SkippedSymlink(target) => {
            println!(
                "A devkit-managed {name} skill is already installed for {who} (left in place):\n  {} -> {}",
                skill_dir.display(),
                target.display()
            );
            println!("It is a symlink; pass --force to replace it with cbc's bundled copy.");
        }
        Outcome::ReplacedSymlink => {
            println!(
                "Replaced the devkit {name} symlink with cbc's bundled skill copy for {who}:\n  {}",
                file.display()
            );
            println!("{restart}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// The canonical single-skill case the original suite was written against: `cbc`,
    /// kept first in [`BUNDLED_SKILLS`]. Most install-mechanics tests drive `install_one`
    /// through this entry; the family-wide behavior is covered separately.
    /// A real file OUTSIDE the skills dir, standing in for the devkit source a symlink
    /// points at. Its contents must survive every install path.
    fn external_target(root: &Path, name: &str) -> (PathBuf, String) {
        let target = root.join(name);
        std::fs::write(&target, "external").unwrap();
        (target, "external".to_string())
    }

    fn cbc_skill() -> &'static BundledSkill {
        let skill = &BUNDLED_SKILLS[0];
        assert_eq!(skill.name, "cbc", "BUNDLED_SKILLS[0] must be the cbc skill");
        skill
    }

    fn install_cbc(skills: &Path, force: bool) -> Outcome {
        install_one(Harness::ClaudeCode, skills, cbc_skill(), force).unwrap()
    }

    // A devkit-style symlinked install: skills/cbc -> <external dir with a source
    // SKILL.md>. Returns (skills_dir, external_source_file). The source file content
    // is a sentinel the corruption-guard assertions check is never mutated.
    fn devkit_symlinked(base: &Path) -> (PathBuf, PathBuf) {
        let skills = base.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        let external = base.join("devkit-cbc");
        std::fs::create_dir_all(&external).unwrap();
        let external_file = external.join("SKILL.md");
        std::fs::write(&external_file, "DEVKIT SOURCE — must not be touched\n").unwrap();
        symlink(&external, skills.join("cbc")).unwrap();
        (skills, external_file)
    }

    #[test]
    fn install_creates_the_skill_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills"); // not pre-created
        let outcome = install_cbc(&skills, false);
        assert_eq!(outcome, Outcome::Created);
        assert_eq!(read(&skills.join("cbc").join("SKILL.md")), cbc_skill().body);
    }

    #[test]
    fn install_is_idempotent_when_content_matches() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        install_cbc(&skills, false); // Created
        let outcome = install_cbc(&skills, false);
        assert_eq!(outcome, Outcome::AlreadyPresent);
        assert!(
            !skills.join("cbc").join("SKILL.md.bak").exists(),
            "an idempotent run must not churn a .bak"
        );
    }

    #[test]
    fn install_updates_and_backs_up_a_stale_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(cbc.join("SKILL.md"), "old stale skill\n").unwrap();

        let outcome = install_cbc(&skills, false);
        assert_eq!(outcome, Outcome::Updated);
        assert_eq!(read(&cbc.join("SKILL.md")), cbc_skill().body);
        assert_eq!(
            read(&cbc.join("SKILL.md.bak")),
            "old stale skill\n",
            "the prior content must survive in the backup"
        );
    }

    #[test]
    fn install_skips_a_symlink_without_force_and_leaves_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = devkit_symlinked(dir.path());

        let outcome = install_cbc(&skills, false);
        match &outcome {
            Outcome::SkippedSymlink(t) => {
                assert_eq!(t, &external_file.parent().unwrap().to_path_buf())
            }
            other => panic!("expected SkippedSymlink, got {other:?}"),
        }
        // THE corruption guard: devkit's source file is byte-for-byte intact.
        assert_eq!(
            read(&external_file),
            "DEVKIT SOURCE — must not be touched\n"
        );
    }

    #[test]
    fn force_replaces_a_symlink_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = devkit_symlinked(dir.path());

        let outcome = install_cbc(&skills, true);
        assert_eq!(outcome, Outcome::ReplacedSymlink);

        let cbc = skills.join("cbc");
        assert!(
            !std::fs::symlink_metadata(&cbc)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the ~/.claude entry must now be a real dir, not a symlink"
        );
        assert_eq!(read(&cbc.join("SKILL.md")), cbc_skill().body);
        // devkit's source is still untouched after the forced replace.
        assert_eq!(
            read(&external_file),
            "DEVKIT SOURCE — must not be touched\n"
        );
    }

    // A real `cbc/` dir whose inner `SKILL.md` is itself a symlink to an external
    // source file. The undocumented-but-possible variant of the corruption vector:
    // the dir is real (so the dir-level lstat guard does NOT fire), yet writing
    // `SKILL.md` would follow the inner link and clobber its target. Returns
    // (skills_dir, external_source_file) so assertions can prove the target is intact.
    fn inner_file_symlinked(base: &Path) -> (PathBuf, PathBuf) {
        let skills = base.join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        let external_file = base.join("external-SKILL.md");
        std::fs::write(&external_file, "EXTERNAL SOURCE — must not be touched\n").unwrap();
        symlink(&external_file, cbc.join("SKILL.md")).unwrap();
        (skills, external_file)
    }

    #[test]
    fn install_skips_an_inner_file_symlink_without_force_and_leaves_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = inner_file_symlinked(dir.path());

        let outcome = install_cbc(&skills, false);
        match &outcome {
            Outcome::SkippedSymlink(t) => assert_eq!(t, &external_file),
            other => panic!("expected SkippedSymlink, got {other:?}"),
        }
        // The corruption guard for the inner-file case: the external target is intact.
        assert_eq!(
            read(&external_file),
            "EXTERNAL SOURCE — must not be touched\n"
        );
    }

    #[test]
    fn force_replaces_an_inner_file_symlink_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = inner_file_symlinked(dir.path());

        let outcome = install_cbc(&skills, true);
        assert_eq!(outcome, Outcome::ReplacedSymlink);

        let skill_file = skills.join("cbc").join("SKILL.md");
        assert!(
            !std::fs::symlink_metadata(&skill_file)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the inner SKILL.md must now be a real file, not a symlink"
        );
        assert_eq!(read(&skill_file), cbc_skill().body);
        // The external target the inner symlink pointed at is still untouched.
        assert_eq!(
            read(&external_file),
            "EXTERNAL SOURCE — must not be touched\n"
        );
    }

    #[test]
    fn install_all_writes_every_bundled_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let outcomes = install_all(Harness::ClaudeCode, &skills, false).unwrap();

        assert_eq!(
            outcomes.len(),
            BUNDLED_SKILLS.len(),
            "install_all must report one outcome per bundled skill"
        );
        for skill in BUNDLED_SKILLS {
            assert_eq!(
                read(&skills.join(skill.name).join("SKILL.md")),
                skill.body,
                "{} must be written verbatim",
                skill.name
            );
        }
        assert!(
            outcomes.iter().all(|(_, o)| *o == Outcome::Created),
            "a fresh install_all should create every skill"
        );

        // And it must be idempotent across the whole family.
        let again = install_all(Harness::ClaudeCode, &skills, false).unwrap();
        assert!(
            again.iter().all(|(_, o)| *o == Outcome::AlreadyPresent),
            "a second install_all must be a no-op for every skill"
        );
    }

    #[test]
    fn every_bundled_skill_carries_its_own_frontmatter() {
        // Guards a wrong include_str! path (a skill pointed at another's body) and a
        // name/frontmatter mismatch. The trailing newline stops `cbc` matching
        // `cbc-orchestrator`'s frontmatter.
        for skill in BUNDLED_SKILLS {
            let expected = format!("---\nname: {}\n", skill.name);
            assert!(
                skill.body.starts_with(&expected),
                "{} must open with `{}`",
                skill.name,
                expected.escape_debug()
            );
            assert!(
                skill.body.contains("\ndescription:"),
                "{} must have a description: field in its frontmatter",
                skill.name
            );
            assert!(
                skill.body.len() > 500,
                "{} skill body must be non-trivial",
                skill.name
            );
        }
    }

    /// Codex discovers a skill by its dir, but only surfaces it as `/name` when the skill
    /// carries `agents/openai.yaml`. That file is DERIVED from the SKILL.md frontmatter
    /// rather than checked in beside it, so the two can never drift apart.
    #[test]
    fn codex_metadata_is_generated_from_the_skill_frontmatter() {
        let body = "---\nname: demo-skill\ndescription: Does a thing well. And a second sentence with more detail.\n---\n\nBody text.\n";
        let yaml = codex_metadata("demo-skill", body).unwrap();

        assert!(
            yaml.contains(r#"display_name: "Demo Skill""#),
            "hyphens become spaces and each word is capitalized: {yaml}"
        );
        assert!(
            yaml.contains(
                r#"short_description: "Does a thing well. And a second sentence with more detail""#
            ),
            "a first sentence under Codex's 25-char floor widens to the whole \
             description rather than being thrown away: {yaml}"
        );
        assert!(
            yaml.contains(r#"default_prompt: "Use $demo-skill to does a thing well.""#),
            "the default prompt invokes the skill explicitly as $name: {yaml}"
        );
    }

    /// Codex needs a second file per skill. Claude Code needs exactly one — writing a
    /// stray `agents/` dir into `~/.claude/skills` would be litter.
    #[test]
    fn a_codex_install_writes_the_skill_and_its_openai_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), false).unwrap();
        assert_eq!(outcome, Outcome::Created);

        let cbc = skills.join("cbc");
        assert_eq!(read(&cbc.join("SKILL.md")), cbc_skill().body);
        let yaml = read(&cbc.join("agents").join("openai.yaml"));
        assert!(
            yaml.contains(r#"default_prompt: "Use $cbc to "#),
            "the Codex metadata must name the skill it launches: {yaml}"
        );
    }

    #[test]
    fn a_claude_install_writes_no_codex_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");

        install_cbc(&skills, false);

        assert!(
            !skills.join("cbc").join("agents").exists(),
            "Claude Code has no use for agents/openai.yaml; it must not be written"
        );
    }

    /// Skill descriptions are prose and pick up abbreviations over time. Splitting on the
    /// first ". " would cut "(e.g. a dead poll)" in half and ship the fragment to Codex.
    #[test]
    fn a_description_that_opens_with_an_abbreviation_keeps_its_whole_first_sentence() {
        let body = "---\nname: demo\ndescription: Use when a room stalls (e.g. a dead poll). Then elaborate at length.\n---\n\nBody text.\n";
        let yaml = codex_metadata("demo", body).unwrap();

        assert!(
            yaml.contains(r#"short_description: "Use when a room stalls (e.g. a dead poll)""#),
            "the sentence must run past the abbreviation: {yaml}"
        );
    }

    /// A Codex skill dir built by an older cbc (SKILL.md only) is NOT up to date: without
    /// `agents/openai.yaml` Codex never surfaces it as `/cbc`. The upgrade path depends on
    /// classify judging the whole file set, not just the SKILL.md.
    #[test]
    fn a_codex_skill_missing_its_metadata_is_updated_not_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        // Exactly what a Claude-shaped install leaves behind.
        install_cbc(&skills, false);

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), false).unwrap();

        assert_eq!(outcome, Outcome::Updated);
        assert!(skills
            .join("cbc")
            .join("agents")
            .join("openai.yaml")
            .exists());
        assert!(
            !skills.join("cbc").join("SKILL.md.bak").exists(),
            "the SKILL.md was already current; backing it up would be churn"
        );
    }

    #[test]
    fn a_codex_install_is_idempotent_across_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        install_one(Harness::Codex, &skills, cbc_skill(), false).unwrap();

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), false).unwrap();

        assert_eq!(outcome, Outcome::AlreadyPresent);
        assert!(
            !skills
                .join("cbc")
                .join("agents")
                .join("openai.yaml.bak")
                .exists(),
            "an idempotent run must not churn a .bak"
        );
    }

    // A real `cbc/` dir whose `agents/` is a symlink to an external dir — the devkit
    // layout, which keeps `agents/openai.yaml` beside its SKILL.md. Writing Codex
    // metadata would follow that link and overwrite devkit's file. Returns
    // (skills_dir, external_source_file).
    fn codex_metadata_symlinked(base: &Path) -> (PathBuf, PathBuf) {
        let skills = base.join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(cbc.join("SKILL.md"), cbc_skill().body).unwrap();
        let external = base.join("devkit-agents");
        std::fs::create_dir_all(&external).unwrap();
        let external_file = external.join("openai.yaml");
        std::fs::write(&external_file, "DEVKIT METADATA — must not be touched\n").unwrap();
        symlink(&external, cbc.join("agents")).unwrap();
        (skills, external_file)
    }

    #[test]
    fn install_skips_a_symlinked_agents_dir_without_force_and_leaves_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = codex_metadata_symlinked(dir.path());

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), false).unwrap();

        match &outcome {
            Outcome::SkippedSymlink(t) => {
                assert_eq!(t, &external_file.parent().unwrap().to_path_buf())
            }
            other => panic!("expected SkippedSymlink, got {other:?}"),
        }
        assert_eq!(
            read(&external_file),
            "DEVKIT METADATA — must not be touched\n"
        );
    }

    #[test]
    fn force_replaces_a_symlinked_agents_dir_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let (skills, external_file) = codex_metadata_symlinked(dir.path());

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), true).unwrap();

        assert_eq!(outcome, Outcome::ReplacedSymlink);
        let agents = skills.join("cbc").join("agents");
        assert!(
            !std::fs::symlink_metadata(&agents)
                .unwrap()
                .file_type()
                .is_symlink(),
            "agents/ must now be a real dir, not a symlink"
        );
        assert!(agents.join("openai.yaml").exists());
        assert_eq!(
            read(&external_file),
            "DEVKIT METADATA — must not be touched\n"
        );
    }

    /// The compile-gate for the generated metadata: every skill CBC ships must yield a
    /// Codex file Codex can actually use. A description that goes missing, or grows a
    /// newline, turns into a broken YAML scalar shipped to every user — caught here
    /// instead of on their machine.
    #[test]
    fn every_bundled_skill_yields_usable_codex_metadata() {
        for skill in BUNDLED_SKILLS {
            let yaml = codex_metadata(skill.name, skill.body)
                .unwrap_or_else(|e| panic!("{} has no usable metadata: {e:#}", skill.name));

            assert_eq!(
                yaml.lines().count(),
                4,
                "{} metadata must be exactly `interface:` plus three single-line fields; \
                 a newline inside a value would break the YAML scalar:\n{yaml}",
                skill.name
            );
            assert!(
                yaml.contains(&format!(r#"default_prompt: "Use ${} to "#, skill.name)),
                "{} must launch itself as $name, the explicit-invocation form Codex's \
                 openai.yaml contract requires: {yaml}",
                skill.name
            );
            let summary = yaml
                .lines()
                .find_map(|l| l.trim().strip_prefix("short_description: "))
                .unwrap()
                .trim_matches('"');
            let len = summary.chars().count();
            assert!(
                (MIN_SHORT..=MAX_SHORT).contains(&len),
                "{}'s short_description is {len} chars; Codex's contract is \
                 {MIN_SHORT}-{MAX_SHORT}: {summary}",
                skill.name
            );
        }
    }

    /// Both harnesses let a user relocate their configuration home, and both then read
    /// skills from the moved home. Installing into a hardcoded `~/.claude` / `~/.codex`
    /// under such a setup writes a directory nobody reads — the skills silently never
    /// appear. This is the only test that touches process env, so it does not race.
    #[test]
    fn a_relocated_configuration_home_is_where_skills_go() {
        for (harness, var) in [
            (Harness::ClaudeCode, "CLAUDE_CONFIG_DIR"),
            (Harness::Codex, "CODEX_HOME"),
        ] {
            let restore = std::env::var_os(var);

            std::env::set_var(var, "/somewhere/else");
            let relocated = harness.skills_dir().unwrap();

            // An empty var is the shell's "unset", not a path relative to the cwd.
            std::env::set_var(var, "");
            let defaulted = harness.skills_dir().unwrap();

            match restore {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }

            assert_eq!(
                relocated,
                PathBuf::from("/somewhere/else").join("skills"),
                "{} must follow {var}",
                harness.label()
            );
            assert!(
                defaulted.starts_with(std::env::var("HOME").unwrap()),
                "an empty {var} must fall back to $HOME, not to a relative path: {}",
                defaulted.display()
            );
        }
    }

    /// `force` promises to replace the link and never touch its target. A skill dir can
    /// hold more than one link, and removing only the first would send the write for the
    /// second straight through it — corrupting the very file the guard exists to protect.
    #[test]
    fn force_replaces_every_symlink_in_one_skill_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(cbc.join("agents")).unwrap();

        let (skill_link_target, _) = external_target(dir.path(), "devkit-SKILL.md");
        let (yaml_link_target, _) = external_target(dir.path(), "devkit-openai.yaml");
        std::os::unix::fs::symlink(&skill_link_target, cbc.join("SKILL.md")).unwrap();
        std::os::unix::fs::symlink(&yaml_link_target, cbc.join("agents").join("openai.yaml"))
            .unwrap();

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), true).unwrap();

        assert_eq!(outcome, Outcome::ReplacedSymlink);
        assert_eq!(
            read(&skill_link_target),
            "external",
            "target must be intact"
        );
        assert_eq!(read(&yaml_link_target), "external", "target must be intact");
        assert_eq!(read(&cbc.join("SKILL.md")), cbc_skill().body);
        assert!(read(&cbc.join("agents").join("openai.yaml")).contains("interface:"));
    }

    /// The backup path gets the same guard as the file it backs up: a symlinked `.bak`
    /// would send the backup write to an unrelated file the installer never inspected.
    #[test]
    fn a_stale_file_is_not_backed_up_through_a_symlinked_bak() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(cbc.join("SKILL.md"), "stale").unwrap();

        let (target, _) = external_target(dir.path(), "unrelated.txt");
        std::os::unix::fs::symlink(&target, cbc.join("SKILL.md.bak")).unwrap();

        let outcome = install_one(Harness::ClaudeCode, &skills, cbc_skill(), false).unwrap();

        assert_eq!(outcome, Outcome::Updated);
        assert_eq!(
            read(&target),
            "external",
            "the symlink's target must be untouched"
        );
        assert_eq!(
            read(&cbc.join("SKILL.md.bak")),
            "stale",
            "the backup must be a real file holding the stale copy"
        );
    }

    /// A blurb Codex's own generator would reject is a blurb its picker will not show.
    #[test]
    fn a_long_description_is_cut_at_a_clause_not_mid_thought() {
        let long = "Coordinate with peer orchestrators as symmetric siblings — open one \
                    pairwise room per peer to reconcile work where your lanes meet";
        let cut = short_description("cbc-peer", long, long);

        assert_eq!(
            cut,
            "Coordinate with peer orchestrators as symmetric siblings"
        );
        assert!((MIN_SHORT..=MAX_SHORT).contains(&cut.chars().count()));
    }

    /// Nothing in a description guarantees a clause boundary inside 64 characters. Cutting
    /// mid-word ("orchestra") reads as a typo; cutting at a word reads as an abbreviation.
    #[test]
    fn a_description_with_no_clause_break_is_cut_at_a_whole_word() {
        let long = "Run a sweep of all worker rooms and flag any that have gone dark now";
        let cut = short_description("cbc-checkup", long, long);

        assert_eq!(
            cut,
            "Run a sweep of all worker rooms and flag any that have gone dark"
        );
        assert!((MIN_SHORT..=MAX_SHORT).contains(&cut.chars().count()));
    }

    /// Cutting at a whole word still lands mid-phrase when that word is a preposition:
    /// "working the same repo at" reads as a truncation bug rather than a summary. The
    /// phrase it opens is gone, so the preposition goes with it.
    #[test]
    fn a_word_truncated_blurb_does_not_end_on_a_dangling_preposition() {
        let at = "Be the orchestrator for multiple agents working the same repo at once \
                  and hold the map of what each is doing";
        let under = "Implement one bounded piece in a repo under orchestrator coordination \
                     and report every status transition";

        assert_eq!(
            short_description("cbc-orchestrator", at, at),
            "Be the orchestrator for multiple agents working the same repo"
        );
        assert_eq!(
            short_description("cbc-worker", under, under),
            "Implement one bounded piece in a repo"
        );
    }

    /// `--force` replaces links; it must not become the one path that destroys a real
    /// file without a copy. A symlinked `agents/` can sit beside a hand-edited
    /// `SKILL.md`, and that stale file is owed the same `.bak` as any other.
    #[test]
    fn force_backs_up_a_stale_real_file_it_finds_behind_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        let cbc = skills.join("cbc");
        std::fs::create_dir_all(&cbc).unwrap();
        std::fs::write(cbc.join("SKILL.md"), "hand-edited").unwrap();

        let agents = dir.path().join("devkit-agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("openai.yaml"), "external").unwrap();
        std::os::unix::fs::symlink(&agents, cbc.join("agents")).unwrap();

        let outcome = install_one(Harness::Codex, &skills, cbc_skill(), true).unwrap();

        assert_eq!(outcome, Outcome::ReplacedSymlink);
        assert_eq!(
            read(&cbc.join("SKILL.md.bak")),
            "hand-edited",
            "the real file the symlink sat beside must survive as a backup"
        );
        assert_eq!(read(&cbc.join("SKILL.md")), cbc_skill().body);
        assert_eq!(
            read(&agents.join("openai.yaml")),
            "external",
            "the symlink's target must be untouched"
        );
    }

    #[test]
    fn bundled_skill_names_are_unique() {
        let mut names: Vec<&str> = BUNDLED_SKILLS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(count, names.len(), "bundled skill names must be unique");
    }
}

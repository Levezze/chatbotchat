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
//! carries `agents/openai.yaml`; without that file the skill dir is inert. That yaml is
//! *derived* from the skill's own frontmatter ([`codex_metadata`]) rather than checked in
//! beside it, so a skill and its Codex metadata cannot drift apart.
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

    /// The harness's home directory. A harness whose home is absent is not installed on
    /// this machine, which is how an automatic install decides whom to write for.
    pub fn home(self) -> anyhow::Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .with_context(|| format!("HOME not set; cannot locate {}", self.label()))?;
        Ok(PathBuf::from(home).join(match self {
            Harness::ClaudeCode => ".claude",
            Harness::Codex => ".codex",
        }))
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
    let prompt = format!("Use the /{name} skill: {summary}");
    Ok(format!(
        "interface:\n  display_name: {}\n  short_description: {}\n  default_prompt: {}\n",
        yaml_quote(&display_name(name)),
        yaml_quote(&summary),
        yaml_quote(&prompt),
    ))
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
            std::fs::remove_file(&link)
                .with_context(|| format!("removing the symlink at {}", link.display()))?;
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
            yaml.contains(r#"short_description: "Does a thing well.""#),
            "only the FIRST sentence of the description is carried: {yaml}"
        );
        assert!(
            yaml.contains(r#"default_prompt: "Use the /demo-skill skill: Does a thing well.""#),
            "the default prompt names the skill and restates its purpose: {yaml}"
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
            yaml.contains(r#"default_prompt: "Use the /cbc skill:"#),
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
            yaml.contains(r#"short_description: "Use when a room stalls (e.g. a dead poll).""#),
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
                yaml.contains(&format!(
                    r#"default_prompt: "Use the /{} skill: "#,
                    skill.name
                )),
                "{} must launch itself by name: {yaml}",
                skill.name
            );
            let summary = yaml
                .lines()
                .find_map(|l| l.trim().strip_prefix("short_description: "))
                .unwrap();
            assert!(
                summary.len() > 20,
                "{} summary is too thin to pick from a list: {summary}",
                skill.name
            );
        }
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

//! `cbc install-skill` — write the bundled Claude Code skills into
//! `~/.claude/skills/<name>/SKILL.md`, so CBC delivers its own agent guidance with no
//! external devkit checkout.
//!
//! CBC ships a small family of skills: `cbc` (drive a room well) plus the orchestration
//! set `cbc-orchestrator` / `cbc-report` / `cbc-peer` / `cbc-recap` / `cbc-reconcile`
//! (coordinate many agents across one or more repos), and `cbc-refresh` (replace a
//! polluted room with a fresh one while preserving the thread). Each skill's text is embedded
//! ([`BUNDLED_SKILLS`] via
//! `include_str!`) so
//! `cargo install` — which ships only the compiled binary — carries them, and the installed
//! skills always match this binary's behavior.
//!
//! Layering mirrors `settings.rs`: a pure classifier ([`classify`]) decides what a target
//! currently is from an injected dir, and the read/back-up/write glue ([`install_one`]) is
//! path-injected so every branch is tested against a tempdir. [`install_all`] just loops the
//! family. The sharp edge it guards: devkit symlinks `~/.claude/skills/<name>` → its own
//! source, so writing through that symlink would corrupt devkit's file. [`classify`] detects
//! a symlink via `symlink_metadata` — at the skill dir itself OR at the inner `SKILL.md` —
//! and [`install_one`] refuses to follow either unless `force` (which removes the link
//! itself, never its target).

use anyhow::Context;
use std::path::{Path, PathBuf};

/// One bundled skill: its directory name under `~/.claude/skills/` (the file is always
/// `SKILL.md` inside it) and its text, embedded at compile time so the binary is
/// self-contained (`cargo install` carries no repo files). chatbotchat is the canonical
/// source of these copies; keep `cbc`'s body in sync with the agent-facing prose in `mcp.rs`.
struct BundledSkill {
    name: &'static str,
    body: &'static str,
}

/// The skills CBC ships and auto-installs. Each lives at `bins/cbc/skill/<name>/SKILL.md`;
/// Claude Code discovers a user-scope skill purely by its `~/.claude/skills/<name>/` path.
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
        name: "cbc-report",
        body: include_str!("../skill/cbc-report/SKILL.md"),
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
];

/// What [`install_one`] did, so the caller can print an honest one-liner.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No skill dir existed; it was created with the bundled SKILL.md.
    Created,
    /// An out-of-date real SKILL.md was refreshed (the prior one backed up to `.bak`).
    Updated,
    /// The on-disk SKILL.md already matched the bundled copy; nothing written.
    AlreadyPresent,
    /// A symlink (the skill dir, devkit-managed, or an inner `SKILL.md`) was found and
    /// `force` was not set, so it was left untouched. Carries the link target.
    SkippedSymlink(PathBuf),
    /// A symlink was found and `force` replaced it with a real bundled copy.
    ReplacedSymlink,
}

/// The current state of `<skills_dir>/<name>`, classified WITHOUT following a symlink.
enum TargetState {
    /// Nothing at the path.
    Absent,
    /// A symlink we must not write *through* — either the skill dir itself
    /// (devkit-managed) or an inner `<name>/SKILL.md`. `link` is the symlink's own path
    /// (what `force` removes); `target` is what it points at (for the message).
    Symlink { link: PathBuf, target: PathBuf },
    /// A real directory whose `SKILL.md` is a real file (or absent); it either matches
    /// the bundled copy or does not (a missing inner file counts as "does not match" →
    /// a rewrite fills it in).
    RealDir { matches: bool },
}

/// `~/.claude/skills` — the Claude Code *user* scope, which applies across every
/// project. Each skill lands at `<this>/<name>/SKILL.md`.
pub fn skills_dir() -> anyhow::Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME not set; cannot locate Claude Code skills")?;
    Ok(PathBuf::from(home).join(".claude").join("skills"))
}

/// Classify `<skill_dir>` against `body` without dereferencing a symlink (the corruption
/// guard). `body` is the bundled text we'd write, so `RealDir { matches }` reports whether
/// the on-disk file already equals it.
fn classify(skill_dir: &Path, body: &str) -> anyhow::Result<TargetState> {
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
    // The skill dir is a real dir. Guard the inner `SKILL.md` too: if it is itself a
    // symlink, a write would follow it and clobber its target. Treat it like the dir case.
    let skill_file = skill_dir.join("SKILL.md");
    if let Ok(m) = std::fs::symlink_metadata(&skill_file) {
        if m.file_type().is_symlink() {
            let target = std::fs::read_link(&skill_file).unwrap_or_else(|_| PathBuf::from("?"));
            return Ok(TargetState::Symlink {
                link: skill_file,
                target,
            });
        }
    }
    let matches = std::fs::read(&skill_file)
        .map(|bytes| bytes == body.as_bytes())
        .unwrap_or(false);
    Ok(TargetState::RealDir { matches })
}

/// Install every bundled skill into `<skills_dir>/<name>/SKILL.md`, returning each skill's
/// name and [`Outcome`] so the caller can print one honest line per skill. Stops at the
/// first hard error (a skills dir we can't read/write), letting callers degrade.
pub fn install_all(skills_dir: &Path, force: bool) -> anyhow::Result<Vec<(&'static str, Outcome)>> {
    let mut outcomes = Vec::with_capacity(BUNDLED_SKILLS.len());
    for skill in BUNDLED_SKILLS {
        let outcome = install_one(skills_dir, skill, force)
            .with_context(|| format!("installing the {} skill", skill.name))?;
        outcomes.push((skill.name, outcome));
    }
    Ok(outcomes)
}

/// Write one bundled skill into `<skills_dir>/<name>/SKILL.md`. Idempotent, backs up a
/// stale file before overwriting, and never writes *through* a devkit symlink unless
/// `force` (which replaces the symlink itself, leaving its target untouched). Errors
/// bubble so callers can degrade rather than crash.
fn install_one(skills_dir: &Path, skill: &BundledSkill, force: bool) -> anyhow::Result<Outcome> {
    let skill_dir = skills_dir.join(skill.name);
    let skill_file = skill_dir.join("SKILL.md");

    match classify(&skill_dir, skill.body)? {
        TargetState::Absent => {
            write_skill(&skill_dir, &skill_file, skill.body)?;
            Ok(Outcome::Created)
        }
        TargetState::RealDir { matches: true } => Ok(Outcome::AlreadyPresent),
        TargetState::RealDir { matches: false } => {
            // Back up a stale file before overwriting (nothing to back up if the dir
            // existed without an inner SKILL.md).
            if skill_file.exists() {
                let backup = PathBuf::from(format!("{}.bak", skill_file.display()));
                let original = std::fs::read(&skill_file)
                    .with_context(|| format!("reading {}", skill_file.display()))?;
                std::fs::write(&backup, &original)
                    .with_context(|| format!("backing up to {}", backup.display()))?;
            }
            write_skill(&skill_dir, &skill_file, skill.body)?;
            Ok(Outcome::Updated)
        }
        TargetState::Symlink { link, target } => {
            if !force {
                return Ok(Outcome::SkippedSymlink(target));
            }
            // Remove the symlink ITSELF (not its target — `remove_file` on a symlink
            // never touches what it points at), then write a real dir + file. `link`
            // is the skill dir for a devkit symlink, or the inner `SKILL.md` for an
            // inner-file symlink; removing either leaves a real dir to write into.
            std::fs::remove_file(&link)
                .with_context(|| format!("removing the symlink at {}", link.display()))?;
            write_skill(&skill_dir, &skill_file, skill.body)?;
            Ok(Outcome::ReplacedSymlink)
        }
    }
}

/// Create the skill dir (idempotent) and write the bundled body to `SKILL.md`.
fn write_skill(skill_dir: &Path, skill_file: &Path, body: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(skill_dir)
        .with_context(|| format!("creating {}", skill_dir.display()))?;
    std::fs::write(skill_file, body)
        .with_context(|| format!("writing {}", skill_file.display()))?;
    Ok(())
}

/// Report what [`install_one`] did for one skill, naming the path explicitly.
pub fn print_outcome(skills_dir: &Path, name: &str, outcome: &Outcome) {
    let skill_dir = skills_dir.join(name);
    let file = skill_dir.join("SKILL.md");
    match outcome {
        Outcome::Created => {
            println!(
                "Installed the {name} skill for Claude Code:\n  {}",
                file.display()
            );
            println!("Start a fresh Claude Code session to pick it up.");
        }
        Outcome::Updated => {
            println!("Updated the {name} skill:\n  {}", file.display());
            println!("(backed up the previous file to {}.bak)", file.display());
            println!("Start a fresh Claude Code session to pick it up.");
        }
        Outcome::AlreadyPresent => {
            println!(
                "The {name} skill at {} is already up to date; nothing to do.",
                file.display()
            );
        }
        Outcome::SkippedSymlink(target) => {
            println!(
                "A devkit-managed {name} skill is already installed (left in place):\n  {} -> {}",
                skill_dir.display(),
                target.display()
            );
            println!("It is a symlink; pass --force to replace it with cbc's bundled copy.");
        }
        Outcome::ReplacedSymlink => {
            println!(
                "Replaced the devkit {name} symlink with cbc's bundled skill copy:\n  {}",
                file.display()
            );
            println!("Start a fresh Claude Code session to pick it up.");
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
        install_one(skills, cbc_skill(), force).unwrap()
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
        let outcomes = install_all(&skills, false).unwrap();

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
        let again = install_all(&skills, false).unwrap();
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

    #[test]
    fn bundled_skill_names_are_unique() {
        let mut names: Vec<&str> = BUNDLED_SKILLS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(count, names.len(), "bundled skill names must be unique");
    }
}

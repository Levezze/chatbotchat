//! `cbc install-skill` — write the bundled `cbc` Claude Code skill into
//! `~/.claude/skills/cbc/SKILL.md`, so CBC delivers its own agent guidance with no
//! external devkit checkout.
//!
//! The skill text is embedded ([`SKILL_BODY`] via `include_str!`) so `cargo install`
//! — which ships only the compiled binary — carries it, and the installed skill always
//! matches this binary's behavior.
//!
//! Layering mirrors `settings.rs`: a pure classifier ([`classify`]) decides what the
//! target currently is from an injected dir, and the read/back-up/write glue
//! ([`install`]) is path-injected so every branch is tested against a tempdir. The
//! sharp edge it guards: devkit symlinks `~/.claude/skills/cbc` → its own source, so
//! writing through that symlink would corrupt devkit's file. [`classify`] detects the
//! symlink via `symlink_metadata` and [`install`] refuses to follow it unless `force`.

use anyhow::Context;
use std::path::{Path, PathBuf};

/// The skill's directory name under `~/.claude/skills/` (the file is `SKILL.md`
/// inside it). Claude Code discovers a user-scope skill purely by this path.
pub const SKILL_NAME: &str = "cbc";

/// The bundled skill text, embedded at compile time so the binary is self-contained
/// (`cargo install` carries no repo files). chatbotchat is the canonical source of
/// this copy; keep it in sync with the agent-facing prose in `mcp.rs`.
const SKILL_BODY: &str = include_str!("../skill/SKILL.md");

/// What [`install`] did, so the caller can print an honest one-liner.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No skill dir existed; it was created with the bundled SKILL.md.
    Created,
    /// An out-of-date real SKILL.md was refreshed (the prior one backed up to `.bak`).
    Updated,
    /// The on-disk SKILL.md already matched the bundled copy; nothing written.
    AlreadyPresent,
    /// The skill dir is a symlink (a devkit-managed install) and `force` was not set,
    /// so it was left untouched. Carries the link target for the message.
    SkippedSymlink(PathBuf),
    /// The skill dir was a symlink and `force` replaced it with a real bundled copy.
    ReplacedSymlink,
}

/// The current state of `<skills_dir>/cbc`, classified WITHOUT following a symlink.
enum TargetState {
    /// Nothing at the path.
    Absent,
    /// The path is a symlink (devkit-managed). Carries its link target.
    Symlink(PathBuf),
    /// A real directory; its `SKILL.md` either matches the bundled copy or does not
    /// (a missing inner file counts as "does not match" → a rewrite fills it in).
    RealDir { matches: bool },
}

/// `~/.claude/skills` — the Claude Code *user* scope, which applies across every
/// project. The skill lands at `<this>/cbc/SKILL.md`.
pub fn skills_dir() -> anyhow::Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME not set; cannot locate Claude Code skills")?;
    Ok(PathBuf::from(home).join(".claude").join("skills"))
}

/// Classify `<skills_dir>/cbc` without dereferencing a symlink (the corruption guard).
fn classify(cbc_dir: &Path) -> anyhow::Result<TargetState> {
    let meta = match std::fs::symlink_metadata(cbc_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TargetState::Absent),
        Err(e) => {
            return Err(e).with_context(|| format!("inspecting {}", cbc_dir.display()));
        }
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(cbc_dir).unwrap_or_else(|_| PathBuf::from("?"));
        return Ok(TargetState::Symlink(target));
    }
    let skill_file = cbc_dir.join("SKILL.md");
    let matches = std::fs::read(&skill_file)
        .map(|bytes| bytes == SKILL_BODY.as_bytes())
        .unwrap_or(false);
    Ok(TargetState::RealDir { matches })
}

/// Write the bundled skill into `<skills_dir>/cbc/SKILL.md`. Idempotent, backs up a
/// stale file before overwriting, and never writes *through* a devkit symlink unless
/// `force` (which replaces the symlink itself, leaving its target untouched). Errors
/// bubble so callers can degrade rather than crash.
pub fn install(skills_dir: &Path, force: bool) -> anyhow::Result<Outcome> {
    let cbc_dir = skills_dir.join(SKILL_NAME);
    let skill_file = cbc_dir.join("SKILL.md");

    match classify(&cbc_dir)? {
        TargetState::Absent => {
            write_skill(&cbc_dir, &skill_file)?;
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
            write_skill(&cbc_dir, &skill_file)?;
            Ok(Outcome::Updated)
        }
        TargetState::Symlink(target) => {
            if !force {
                return Ok(Outcome::SkippedSymlink(target));
            }
            // Remove the symlink ITSELF (not its target — `remove_file` on a symlink
            // never touches what it points at), then write a real dir + file.
            std::fs::remove_file(&cbc_dir)
                .with_context(|| format!("removing the symlink at {}", cbc_dir.display()))?;
            write_skill(&cbc_dir, &skill_file)?;
            Ok(Outcome::ReplacedSymlink)
        }
    }
}

/// Create the skill dir (idempotent) and write the bundled body to `SKILL.md`.
fn write_skill(cbc_dir: &Path, skill_file: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(cbc_dir).with_context(|| format!("creating {}", cbc_dir.display()))?;
    std::fs::write(skill_file, SKILL_BODY)
        .with_context(|| format!("writing {}", skill_file.display()))?;
    Ok(())
}

/// Report what [`install`] did, naming the path explicitly.
pub fn print_outcome(skills_dir: &Path, outcome: &Outcome) {
    let cbc_dir = skills_dir.join(SKILL_NAME);
    let file = cbc_dir.join("SKILL.md");
    match outcome {
        Outcome::Created => {
            println!(
                "Installed the cbc skill for Claude Code:\n  {}",
                file.display()
            );
            println!("Start a fresh Claude Code session to pick it up.");
        }
        Outcome::Updated => {
            println!("Updated the cbc skill:\n  {}", file.display());
            println!("(backed up the previous file to {}.bak)", file.display());
            println!("Start a fresh Claude Code session to pick it up.");
        }
        Outcome::AlreadyPresent => {
            println!(
                "The cbc skill at {} is already up to date; nothing to do.",
                file.display()
            );
        }
        Outcome::SkippedSymlink(target) => {
            println!(
                "A devkit-managed cbc skill is already installed (left in place):\n  {} -> {}",
                cbc_dir.display(),
                target.display()
            );
            println!("It is a symlink; pass --force to replace it with cbc's bundled copy.");
        }
        Outcome::ReplacedSymlink => {
            println!(
                "Replaced the devkit symlink with cbc's bundled skill copy:\n  {}",
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
        let outcome = install(&skills, false).unwrap();
        assert_eq!(outcome, Outcome::Created);
        assert_eq!(read(&skills.join("cbc").join("SKILL.md")), SKILL_BODY);
    }

    #[test]
    fn install_is_idempotent_when_content_matches() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        install(&skills, false).unwrap(); // Created
        let outcome = install(&skills, false).unwrap();
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

        let outcome = install(&skills, false).unwrap();
        assert_eq!(outcome, Outcome::Updated);
        assert_eq!(read(&cbc.join("SKILL.md")), SKILL_BODY);
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

        let outcome = install(&skills, false).unwrap();
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

        let outcome = install(&skills, true).unwrap();
        assert_eq!(outcome, Outcome::ReplacedSymlink);

        let cbc = skills.join("cbc");
        assert!(
            !std::fs::symlink_metadata(&cbc)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the ~/.claude entry must now be a real dir, not a symlink"
        );
        assert_eq!(read(&cbc.join("SKILL.md")), SKILL_BODY);
        // devkit's source is still untouched after the forced replace.
        assert_eq!(
            read(&external_file),
            "DEVKIT SOURCE — must not be touched\n"
        );
    }

    #[test]
    fn the_embedded_skill_is_a_valid_cbc_skill() {
        assert!(
            SKILL_BODY.starts_with("---\nname: cbc"),
            "embedded SKILL.md must carry the cbc frontmatter (guards a wrong include_str! path)"
        );
        assert!(SKILL_BODY.len() > 500, "embedded skill must be non-trivial");
    }
}

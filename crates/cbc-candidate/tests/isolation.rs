use assert_cmd::Command;
use std::fs;
use tempfile::tempdir;

fn candidate(temp: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("cbc-candidate").unwrap();
    command
        .env("HOME", temp.join("home"))
        .env_remove("CBC_DB")
        .env_remove("CBC_PORT")
        .env_remove("CBC_SERVER");
    command
}

#[test]
fn initializes_a_private_namespace_without_opening_a_database_or_starting_a_server() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    fs::create_dir(protected.parent().unwrap()).unwrap();
    fs::write(&protected, b"daily sentinel").unwrap();

    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();

    assert!(root.join("candidate.json").is_file());
    assert!(!root.join("candidate.sqlite3").exists());
    assert_eq!(fs::read(&protected).unwrap(), b"daily sentinel");
}

#[test]
fn refuses_the_daily_port_before_creating_any_candidate_files() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8484", "--protected-db"])
        .arg(temp.path().join("daily.db"))
        .assert()
        .failure();
    assert!(!root.exists());
}

#[test]
fn refuses_a_sandbox_inside_the_protected_database_directory() {
    let temp = tempdir().unwrap();
    let daily = temp.path().join("daily");
    fs::create_dir(&daily).unwrap();
    let protected = daily.join("state.db");
    fs::write(&protected, b"do not touch").unwrap();
    let root = daily.join("candidate");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .failure();
    assert!(!root.exists());
    assert_eq!(fs::read(protected).unwrap(), b"do not touch");
}

#[test]
fn inspects_an_existing_candidate_without_creating_the_database() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();
    let before = fs::read(root.join("candidate.json")).unwrap();
    candidate(temp.path())
        .args(["inspect", "--root"])
        .arg(&root)
        .arg("--protected-db")
        .arg(&protected)
        .assert()
        .success();
    assert_eq!(fs::read(root.join("candidate.json")).unwrap(), before);
    assert!(!root.join("candidate.sqlite3").exists());
}

#[test]
fn rejects_a_daily_database_hardlink_without_modifying_it() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    fs::create_dir(protected.parent().unwrap()).unwrap();
    fs::write(&protected, b"daily sentinel").unwrap();
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();
    fs::hard_link(&protected, root.join("candidate.sqlite3")).unwrap();
    candidate(temp.path())
        .args(["inspect", "--root"])
        .arg(&root)
        .arg("--protected-db")
        .arg(&protected)
        .assert()
        .failure();
    assert_eq!(fs::read(protected).unwrap(), b"daily sentinel");
}

#[test]
fn refuses_a_candidate_root_readable_by_other_users() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    candidate(temp.path())
        .args(["inspect", "--root"])
        .arg(&root)
        .arg("--protected-db")
        .arg(&protected)
        .assert()
        .failure();
}

#[test]
fn extra_protected_paths_cannot_replace_the_home_default() {
    let temp = tempdir().unwrap();
    let daily = temp.path().join("home/.chatbotchat");
    fs::create_dir_all(&daily).unwrap();
    let root = daily.join("candidate");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(temp.path().join("extra/state.db"))
        .assert()
        .failure();
    assert!(!root.exists());
}

#[test]
fn legacy_environment_only_adds_protection_never_selects_candidate_storage() {
    let temp = tempdir().unwrap();
    let daily = temp.path().join("relocated");
    fs::create_dir(&daily).unwrap();
    let root = daily.join("candidate");
    candidate(temp.path())
        .env("CBC_DB", daily.join("state.db"))
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(temp.path().join("extra/state.db"))
        .assert()
        .failure();
    assert!(!root.exists());
}

#[test]
fn also_protects_relocated_live_ports_from_legacy_configuration() {
    for (key, value) in [("CBC_PORT", "8984"), ("CBC_SERVER", "http://[::1]:8984")] {
        let temp = tempdir().unwrap();
        let root = temp.path().join("candidate");
        candidate(temp.path())
            .env(key, value)
            .args(["init", "--root"])
            .arg(&root)
            .args(["--port", "8984", "--protected-db"])
            .arg(temp.path().join("daily/state.db"))
            .assert()
            .failure();
        assert!(!root.exists());
    }
}

#[test]
fn refuses_an_existing_foreign_database_in_the_candidate_namespace() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();
    let db = root.join("candidate.sqlite3");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&db)
        .unwrap()
        .write_all(b"foreign database sentinel")
        .unwrap();
    candidate(temp.path())
        .args(["inspect", "--root"])
        .arg(&root)
        .arg("--protected-db")
        .arg(&protected)
        .assert()
        .failure();
    assert_eq!(fs::read(db).unwrap(), b"foreign database sentinel");
}

#[test]
fn protects_the_real_directory_when_the_daily_database_is_a_symlink() {
    use std::os::unix::fs::symlink;
    let temp = tempdir().unwrap();
    let daily = temp.path().join("daily");
    let actual = temp.path().join("actual");
    fs::create_dir(&daily).unwrap();
    fs::create_dir(&actual).unwrap();
    fs::write(actual.join("state.db"), b"daily sentinel").unwrap();
    let protected = daily.join("state.db");
    symlink(actual.join("state.db"), &protected).unwrap();
    let root = actual.join("candidate");
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .failure();
    assert!(!root.exists());
}

#[test]
fn rejects_symlinked_manifest_database_and_sidecars_before_reading_them() {
    use std::os::unix::fs::symlink;
    for name in [
        "candidate.json",
        "candidate.sqlite3",
        "candidate.sqlite3-wal",
        "candidate.sqlite3-shm",
    ] {
        let temp = tempdir().unwrap();
        let root = temp.path().join("candidate");
        let protected = temp.path().join("daily/state.db");
        fs::create_dir(protected.parent().unwrap()).unwrap();
        fs::write(&protected, b"daily sentinel").unwrap();
        candidate(temp.path())
            .args(["init", "--root"])
            .arg(&root)
            .args(["--port", "8984", "--protected-db"])
            .arg(&protected)
            .assert()
            .success();
        if name == "candidate.json" {
            fs::remove_file(root.join(name)).unwrap();
        }
        symlink(&protected, root.join(name)).unwrap();
        candidate(temp.path())
            .args(["inspect", "--root"])
            .arg(&root)
            .arg("--protected-db")
            .arg(&protected)
            .assert()
            .failure();
        assert_eq!(fs::read(&protected).unwrap(), b"daily sentinel");
    }
}

#[test]
fn refuses_existing_directories_and_does_not_overwrite_their_contents() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("existing");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep"), b"keep").unwrap();
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(temp.path().join("daily/state.db"))
        .assert()
        .failure();
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"keep");
    assert!(!root.join("candidate.json").exists());
}

#[test]
fn rejects_foreign_corrupt_or_oversized_manifests_without_repairing_them() {
    for replacement in [b"not JSON".to_vec(), vec![b' '; 4097]] {
        let temp = tempdir().unwrap();
        let root = temp.path().join("candidate");
        let protected = temp.path().join("daily/state.db");
        candidate(temp.path())
            .args(["init", "--root"])
            .arg(&root)
            .args(["--port", "8984", "--protected-db"])
            .arg(&protected)
            .assert()
            .success();
        fs::write(root.join("candidate.json"), &replacement).unwrap();
        candidate(temp.path())
            .args(["inspect", "--root"])
            .arg(&root)
            .arg("--protected-db")
            .arg(&protected)
            .assert()
            .failure();
        assert_eq!(fs::read(root.join("candidate.json")).unwrap(), replacement);
    }
}

#[test]
fn rejects_relative_and_parent_traversing_roots() {
    let temp = tempdir().unwrap();
    for root in [
        std::path::PathBuf::from("candidate"),
        temp.path().join("../candidate"),
    ] {
        candidate(temp.path())
            .current_dir(temp.path())
            .args(["init", "--root"])
            .arg(root)
            .args(["--port", "8984", "--protected-db"])
            .arg(temp.path().join("daily/state.db"))
            .assert()
            .failure();
    }
}

#[test]
fn overriding_home_cannot_remove_the_os_account_default_from_protection() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let output = candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(temp.path().join("daily/state.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let manifest: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let actual = cbc_candidate::account_home()
        .unwrap()
        .join(".chatbotchat/state.db");
    assert!(manifest["protected_databases"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(actual)));
}

#[test]
fn rejects_rollback_journal_aliases_too() {
    use std::os::unix::fs::symlink;
    let temp = tempdir().unwrap();
    let root = temp.path().join("candidate");
    let protected = temp.path().join("daily/state.db");
    fs::create_dir(protected.parent().unwrap()).unwrap();
    fs::write(&protected, b"daily sentinel").unwrap();
    candidate(temp.path())
        .args(["init", "--root"])
        .arg(&root)
        .args(["--port", "8984", "--protected-db"])
        .arg(&protected)
        .assert()
        .success();
    symlink(&protected, root.join("candidate.sqlite3-journal")).unwrap();
    candidate(temp.path())
        .args(["inspect", "--root"])
        .arg(&root)
        .arg("--protected-db")
        .arg(&protected)
        .assert()
        .failure();
    assert_eq!(fs::read(&protected).unwrap(), b"daily sentinel");
}

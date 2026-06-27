use std::fs;

use assert_cmd::Command;
use predicates::str::contains;

fn bin() -> Command {
    Command::cargo_bin("zoxide").unwrap()
}

fn add_dir(data_dir: &std::path::Path, path: &std::path::Path) {
    bin()
        .env("_ZO_DATA_DIR", data_dir)
        .arg("add")
        .arg(path)
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn query_typo_fallback_preserves_normal_match_and_can_be_disabled() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let target = root.path().join("home/lewis/xfce4-terminal");
    let launcher = root.path().join("home/lewis/applicationlauncher");
    let config = root.path().join("home/lewis/tasks/config");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&launcher).unwrap();
    fs::create_dir_all(&config).unwrap();
    add_dir(&data_dir, &target);
    add_dir(&data_dir, &launcher);
    add_dir(&data_dir, &config);

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("xfce4-terminal")
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("xfce4-terinal")
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("xgce4-tremriianl")
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("zgce4")
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("xzfce-ter")
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .args(["query", "--list", "zgce4", "terminal"])
        .assert()
        .success()
        .stdout(format!("{}\n", target.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("app")
        .arg("laucnh")
        .assert()
        .success()
        .stdout(format!("{}\n", launcher.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("applaunch")
        .assert()
        .success()
        .stdout(format!("{}\n", launcher.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("tasks")
        .arg("cinfig")
        .assert()
        .success()
        .stdout(format!("{}\n", config.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .env("_ZO_TYPO_FALLBACK", "0")
        .arg("query")
        .arg("xfce4-terinal")
        .assert()
        .failure()
        .stderr(contains("no match found"));
}

#[test]
fn interactive_query_shows_exact_and_typo_distances() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let exact = root.path().join("home/lewis/xfce4-terminal");
    let near = root.path().join("home/lewis/xfce4-terminao");
    fs::create_dir_all(&exact).unwrap();
    fs::create_dir_all(&near).unwrap();
    add_dir(&data_dir, &exact);
    add_dir(&data_dir, &near);

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .env("_ZO_FZF_OPTS", "--filter=xfce4-term")
        .args(["query", "--interactive", "--score", "xfce4", "terminal"])
        .assert()
        .success()
        .stdout(contains("p=0  m=0  d=0 "))
        .stdout(contains(format!("\t{}", exact.display())))
        .stdout(contains("p=0  m=0  d=1 "))
        .stdout(contains(format!("\t{}", near.display())));
}

#[test]
fn query_typo_fallback_honors_exclude_and_base_dir() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let inside = root.path().join("workspace/inside/xfce4-terminal");
    let outside = root.path().join("workspace/outside/xfce4-terminal");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    add_dir(&data_dir, &inside);
    add_dir(&data_dir, &outside);

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("--exclude")
        .arg(&inside)
        .arg("xfce4-terinal")
        .assert()
        .success()
        .stdout(format!("{}\n", outside.display()));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("--base-dir")
        .arg(root.path().join("workspace/inside"))
        .arg("xfce4-terinal")
        .assert()
        .success()
        .stdout(format!("{}\n", inside.display()));
}

#[test]
fn normal_query_prefers_higher_frecency_before_path_position() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let exact = root.path().join("home/lewis/tasks/config");
    let suffix = root.path().join("home/lewis/tasks/redragonmouseconfig");
    fs::create_dir_all(&exact).unwrap();
    fs::create_dir_all(&suffix).unwrap();
    add_dir(&data_dir, &exact);
    for _ in 0..8 {
        add_dir(&data_dir, &suffix);
    }

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .args(["query", "--list", "tasks", "config"])
        .assert()
        .success()
        .stdout(format!("{}\n{}\n", suffix.display(), exact.display()));
}

#[test]
fn normal_query_uses_lower_match_penalty_as_last_tiebreak() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let lower_penalty = root.path().join("home/lewis/tasks/config");
    let higher_penalty = root.path().join("home/lewis/tasks/redragonmouseconfig");
    fs::create_dir_all(&lower_penalty).unwrap();
    fs::create_dir_all(&higher_penalty).unwrap();
    add_dir(&data_dir, &lower_penalty);
    add_dir(&data_dir, &higher_penalty);

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .args(["query", "--list", "tasks", "onfig"])
        .assert()
        .success()
        .stdout(format!("{}\n{}\n", lower_penalty.display(), higher_penalty.display()));
}

#[test]
fn query_typo_fallback_ignores_missing_paths_unless_all_is_set() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let missing = root.path().join("home/lewis/other-terminal");
    fs::create_dir_all(&missing).unwrap();
    add_dir(&data_dir, &missing);
    fs::remove_dir_all(&missing).unwrap();

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("other-terinal")
        .assert()
        .failure()
        .stderr(contains("no match found"));

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .arg("query")
        .arg("--all")
        .arg("other-terinal")
        .assert()
        .success()
        .stdout(format!("{}\n", missing.display()));
}

#[test]
fn typo_fallback_can_show_zero_distance_matches_in_non_basename_components() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let basename = root.path().join("home/lewis/Dev/applicationlauncher");
    let nested = root.path().join("home/lewis/Dev/applicationlauncher/target/release");
    fs::create_dir_all(&basename).unwrap();
    fs::create_dir_all(&nested).unwrap();
    add_dir(&data_dir, &basename);
    add_dir(&data_dir, &nested);

    bin()
        .env("_ZO_DATA_DIR", &data_dir)
        .args(["query", "--list", "ap", "laun"])
        .assert()
        .success()
        .stdout(format!("{}\n", basename.display()));
}

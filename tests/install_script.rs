//! The release installer script must at least parse, and be shellcheck-clean
//! wherever shellcheck is available.

use std::process::Command;

fn script() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn install_sh_parses() {
    let out = Command::new("sh").arg("-n").arg(script()).output().unwrap();
    assert!(
        out.status.success(),
        "sh -n install.sh: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn install_sh_is_shellcheck_clean_when_shellcheck_exists() {
    if !have("shellcheck") {
        eprintln!("shellcheck not installed; sh -n covers the syntax");
        return;
    }
    let out = Command::new("shellcheck")
        .args(["-s", "sh"])
        .arg(script())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn install_sh_is_strict_and_runs_setup() {
    let body = std::fs::read_to_string(script()).unwrap();
    assert!(body.contains("set -eu"));
    assert!(body.contains("setup --yes"));
    assert!(body.contains("PERCH_VERSION"));
    assert!(body.contains("--no-setup"));
    assert!(body.contains("the tarball contained no perch binary"));
}

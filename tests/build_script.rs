#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    checkout: PathBuf,
    cwd: PathBuf,
    bin: PathBuf,
    captured: PathBuf,
}

impl Fixture {
    fn new(with_cargo: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lyrics fetcher 构建 {}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let checkout = root.join("checkout space 中文 'quote'");
        let cwd = root.join("other working dir 其他");
        let bin = root.join("fake tools");
        let captured = root.join("cargo arguments");
        for path in [&checkout, &cwd, &bin] {
            fs::create_dir_all(path).unwrap();
        }
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("build.sh"),
            checkout.join("build.sh"),
        )
        .unwrap();
        executable(
            &bin.join("dirname"),
            "#!/bin/sh\ncase \"$1\" in\n  */*) printf '%s\\n' \"${1%/*}\" ;;\n  *) printf '.\\n' ;;\nesac\n",
        );
        if with_cargo {
            executable(
                &bin.join("cargo"),
                "#!/bin/sh\nprintf '%s\\000' \"$@\" > \"$BUILD_SCRIPT_ARGUMENTS\"\nexit \"${BUILD_SCRIPT_STATUS:-0}\"\n",
            );
        }
        Self {
            root,
            checkout,
            cwd,
            bin,
            captured,
        }
    }

    fn run(&self, shell: &Path, args: &[OsString], status: &str) -> Output {
        Command::new(shell)
            .arg(self.checkout.join("build.sh"))
            .args(args)
            .current_dir(&self.cwd)
            .env("PATH", &self.bin)
            .env("CDPATH", "a nonexistent directory")
            .env("BUILD_SCRIPT_ARGUMENTS", &self.captured)
            .env("BUILD_SCRIPT_STATUS", status)
            .output()
            .unwrap()
    }

    fn expected(&self, extra: &[OsString]) -> Vec<Vec<u8>> {
        let mut args = vec![
            OsString::from("build"),
            OsString::from("--manifest-path"),
            fs::canonicalize(&self.checkout)
                .unwrap()
                .join("Cargo.toml")
                .into_os_string(),
            OsString::from("--locked"),
            OsString::from("--release"),
            OsString::from("--bin"),
            OsString::from("lyrics-fetcher"),
        ];
        args.extend_from_slice(extra);
        args.iter().map(|arg| arg.as_bytes().to_vec()).collect()
    }

    fn captured(&self) -> Vec<Vec<u8>> {
        let bytes = fs::read(&self.captured).unwrap();
        assert_eq!(bytes.last(), Some(&0), "arguments must be NUL-delimited");
        bytes[..bytes.len() - 1]
            .split(|byte| *byte == 0)
            .map(<[u8]>::to_vec)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn find_shell(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

fn verify_shell(shell: &Path) {
    let fixture = Fixture::new(true);
    let output = fixture.run(shell, &[], "0");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.captured(), fixture.expected(&[]));

    let extra = vec![
        OsString::from("--offline"),
        OsString::from("--target-dir"),
        OsString::from("输出 directory with spaces"),
        OsString::from(""),
        OsString::from("literal * $HOME ; 'quoted'"),
    ];
    let output = fixture.run(shell, &extra, "0");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.captured(), fixture.expected(&extra));

    let output = fixture.run(shell, &[OsString::from("--offline")], "37");
    assert_eq!(output.status.code(), Some(37));
    assert_eq!(
        fixture.captured(),
        fixture.expected(&[OsString::from("--offline")])
    );

    let missing = Fixture::new(false);
    let output = missing.run(shell, &[], "0");
    assert_eq!(output.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cargo was not found in PATH"));
    assert!(!missing.captured.exists());
}

#[test]
fn sh_preserves_arguments_paths_and_exit_status() {
    verify_shell(&find_shell("sh").expect("POSIX sh is required for Unix builds"));
}

#[test]
fn bash_preserves_arguments_paths_and_exit_status_when_available() {
    match find_shell("bash") {
        Some(shell) => verify_shell(&shell),
        None => eprintln!("skipped: bash is not available in PATH"),
    }
}

#[test]
fn build_script_is_executable_and_runs_directly() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("build.sh");
    assert!(fs::metadata(&script).unwrap().permissions().mode() & 0o111 != 0);
    let fixture = Fixture::new(true);
    let output = Command::new(fixture.checkout.join("build.sh"))
        .current_dir(&fixture.cwd)
        .env("PATH", &fixture.bin)
        .env("BUILD_SCRIPT_ARGUMENTS", &fixture.captured)
        .env("BUILD_SCRIPT_STATUS", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.captured(), fixture.expected(&[]));
}

#[test]
fn sh_preserves_non_utf8_cargo_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new(true);
    let extra = [OsString::from_vec(b"output \xff directory".to_vec())];
    let output = fixture.run(&find_shell("sh").unwrap(), &extra, "0");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.captured(), fixture.expected(&extra));
}

#[test]
fn build_script_does_not_create_a_custom_debug_protocol() {
    let fixture = Fixture::new(true);
    let extra = [OsStr::new("--debug").to_os_string()];
    let output = fixture.run(&find_shell("sh").unwrap(), &extra, "0");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fixture.captured(), fixture.expected(&extra));
}

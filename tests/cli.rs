#![cfg(feature = "cli")]

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lyrics-fetcher"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_documents_short_long_and_input_modes() {
    for option in ["--help", "-h"] {
        let output = run(&[option]);
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("--lyrics") && help.contains("-l"));
        assert!(help.contains("--input-type") && help.contains("--select"));
    }
    assert!(run(&["--version"]).status.success());
}

#[test]
fn malformed_inputs_do_not_make_requests() {
    for flag in ["-l", "--lyrics"] {
        for input in [
            "",
            "spotify:1",
            "https://127.0.0.1/song?id=1",
            "netease:bad",
            "./missing.flac",
        ] {
            let output = run(&[flag, input]);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{flag} {input}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    assert_eq!(run(&[]).status.code(), Some(2));
    assert_eq!(run(&["-l"]).status.code(), Some(2));
    assert_eq!(
        run(&["-l", "netease:1", "--provider", "qq"]).status.code(),
        Some(2)
    );
}

#[test]
fn existing_output_is_unchanged_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("中文 existing.lrc");
    std::fs::write(&path, b"keep existing").unwrap();
    for flag in ["-l", "--lyrics"] {
        let output = Command::new(env!("CARGO_BIN_EXE_lyrics-fetcher"))
            .args([flag, "netease:1", "--output"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(5),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"keep existing");
    }
}

#[test]
fn bad_audio_reports_local_error_without_network() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad audio.flac");
    std::fs::write(&path, b"not audio").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_lyrics-fetcher"))
        .arg("-l")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!path.with_extension("lrc").exists());
}

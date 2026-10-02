use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

fn unsupported_name(path: &Path, error: &io::Error) -> bool {
    // macOS EILSEQ: some native filesystems reject non-UTF-8 names.
    cfg!(target_os = "macos")
        && path.as_os_str().to_str().is_none()
        && error.raw_os_error() == Some(92)
}

pub(crate) fn create_fixture(scenario: &str, path: &Path, contents: &[u8]) -> io::Result<bool> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if unsupported_name(path, &error) => {
            writeln!(
                io::stderr().lock(),
                "skipped {scenario}: non-UTF-8 filesystem fixture {path:?}: {error}"
            )?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    file.write_all(contents)?;
    Ok(true)
}

#[test]
fn only_macos_eilseq_for_non_utf8_names_is_unsupported() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let native = OsString::from_vec(b"track\xff.lrc".to_vec());
    let path = Path::new(&native);
    assert_eq!(
        unsupported_name(path, &io::Error::from_raw_os_error(92)),
        cfg!(target_os = "macos")
    );
    assert!(!unsupported_name(
        Path::new("track.lrc"),
        &io::Error::from_raw_os_error(92)
    ));
    for kind in [
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::AlreadyExists,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::InvalidFilename,
        io::ErrorKind::NotFound,
        io::ErrorKind::Other,
    ] {
        assert!(!unsupported_name(path, &io::Error::from(kind)));
    }
}

#[test]
fn fixture_creation_writes_without_clobbering_and_propagates_other_errors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture");
    assert!(create_fixture("helper_behavior", &path, b"keep").unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    assert_eq!(
        create_fixture("helper_behavior", &path, b"overwrite")
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    let missing = directory.path().join("absent");
    assert_eq!(
        create_fixture("helper_behavior", &missing.join("fixture"), b"data")
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    let link = directory.path().join("dangling");
    std::os::unix::fs::symlink(&missing, &link).unwrap();
    assert_eq!(
        create_fixture("helper_behavior", &link, b"overwrite")
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), missing);
    assert!(!missing.exists());
}

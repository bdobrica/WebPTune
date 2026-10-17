//! Output replacement and process-level I/O failures.

use std::fs;
use std::process::Command;

use tempfile::tempdir;

#[cfg(unix)]
const SOURCE: &[u8] = include_bytes!("fixtures/rgb.png");

#[cfg(unix)]
#[test]
fn replacing_unrelated_symlink_replaces_entry_and_preserves_referent() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let input = directory.path().join("input.png");
    let referent = directory.path().join("other.webp");
    let destination = directory.path().join("output.webp");
    fs::write(&input, SOURCE).unwrap();
    fs::write(&referent, b"unrelated file").unwrap();
    symlink(&referent, &destination).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    assert!(result.stdout.is_empty());
    assert!(!fs::symlink_metadata(&destination)
        .unwrap()
        .file_type()
        .is_symlink());
    image::load_from_memory_with_format(&fs::read(&destination).unwrap(), image::ImageFormat::WebP)
        .unwrap();
    assert_eq!(fs::read(&referent).unwrap(), b"unrelated file");
    assert_eq!(fs::read(&input).unwrap(), SOURCE);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
}

#[cfg(unix)]
#[test]
fn broken_stdout_returns_nonzero_and_preserves_source() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::Stdio;

    let directory = tempdir().unwrap();
    let input = directory.path().join("input.png");
    fs::write(&input, SOURCE).unwrap();
    // Close the reader before spawning, so even the smallest WebP reliably
    // encounters EPIPE instead of racing a parent that closes a normal pipe.
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let writer: OwnedFd = writer.into();
    let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
        .arg("-i")
        .arg(&input)
        .args(["-o", "-"])
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("cannot write WebP to stdout"), "{stderr}");
    assert!(!stderr.contains("Wrote WebP"));
    assert_eq!(fs::read(&input).unwrap(), SOURCE);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn invalid_input_preserves_destination_and_leaves_no_temporary_output() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("bad.png");
    let destination = directory.path().join("output.webp");
    fs::write(&input, b"invalid image").unwrap();
    fs::write(&destination, b"existing destination").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert_eq!(fs::read(&input).unwrap(), b"invalid image");
    assert_eq!(fs::read(&destination).unwrap(), b"existing destination");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

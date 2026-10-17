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

// Use the OS temporary directory so these exercise native Unix permissions,
// even when the checkout is on a filesystem that ignores chmod (e.g. DrvFs).
#[cfg(unix)]
mod permissions {
    use super::*;
    use std::fs::{File, OpenOptions, Permissions};
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Output;

    struct RestorePermissions(PathBuf, Permissions);

    impl RestorePermissions {
        fn set(path: &Path, mode: u32) -> Self {
            let original = fs::metadata(path).unwrap().permissions();
            fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
            Self(path.to_owned(), original)
        }
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            // Restore on assertion failures too, allowing TempDir cleanup.
            fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }

    fn assert_permission_failure(result: &Output, context: &str) {
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(context), "{stderr}");
        assert!(!stderr.contains("Wrote WebP"), "{stderr}");
    }

    #[test]
    fn unreadable_input_preserves_source_and_existing_or_absent_destination() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input.png");
        let destination = directory.path().join("output.webp");
        fs::write(&input, SOURCE).unwrap();
        let restore = RestorePermissions::set(&input, 0o000);
        match File::open(&input) {
            Err(error) => assert_eq!(error.kind(), ErrorKind::PermissionDenied),
            Ok(_) => {
                eprintln!("skipping unreadable-input check: this user/filesystem bypasses Unix read permissions");
                return;
            }
        }
        for existing in [false, true] {
            if existing {
                fs::write(&destination, b"existing destination").unwrap();
            }
            for target in [destination.as_os_str(), std::ffi::OsStr::new("-")] {
                let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
                    .arg("-i")
                    .arg(&input)
                    .arg("-o")
                    .arg(target)
                    .output()
                    .unwrap();
                assert_permission_failure(&result, "cannot read input");
                if existing {
                    assert_eq!(fs::read(&destination).unwrap(), b"existing destination");
                } else {
                    assert!(!destination.exists());
                }
                assert_eq!(
                    fs::read_dir(directory.path()).unwrap().count(),
                    1 + usize::from(existing)
                );
            }
        }
        drop(restore);
        assert_eq!(fs::read(&input).unwrap(), SOURCE);
    }

    #[test]
    fn unwritable_output_directory_preserves_source_destination_and_no_staging_files() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("input.png");
        let parent = directory.path().join("locked");
        let destination = parent.join("output.webp");
        fs::write(&input, SOURCE).unwrap();
        fs::create_dir(&parent).unwrap();
        for existing in [false, true] {
            if existing {
                fs::write(&destination, b"existing destination").unwrap();
            }
            let restore = RestorePermissions::set(&parent, 0o555);
            let probe = parent.join("permission-probe");
            match OpenOptions::new().write(true).create_new(true).open(&probe) {
                Err(error) => assert_eq!(error.kind(), ErrorKind::PermissionDenied),
                Ok(file) => {
                    drop(file);
                    drop(restore);
                    fs::remove_file(&probe).unwrap();
                    eprintln!("skipping unwritable-directory check: this user/filesystem bypasses Unix write permissions");
                    return;
                }
            }
            let result = Command::new(env!("CARGO_BIN_EXE_webp-ssim-optimizer"))
                .arg("-i")
                .arg(&input)
                .arg("-o")
                .arg(&destination)
                .output()
                .unwrap();
            drop(restore);
            assert_permission_failure(&result, "cannot create temporary output");
            assert_eq!(fs::read(&input).unwrap(), SOURCE);
            if existing {
                assert_eq!(fs::read(&destination).unwrap(), b"existing destination");
            } else {
                assert!(!destination.exists());
            }
            assert_eq!(
                fs::read_dir(&parent).unwrap().count(),
                usize::from(existing)
            );
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        }
    }
}

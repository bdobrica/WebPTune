use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::{tempdir, TempDir};

const SOURCE: &[u8] = include_bytes!("fixtures/rgb.png");
const DESTINATION: &[u8] = b"existing destination must survive validation";

struct Fixture {
    dir: TempDir,
    input: PathBuf,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let input = dir.path().join("source.png");
        let output = dir.path().join("destination.webp");
        fs::write(&input, SOURCE).unwrap();
        Self { dir, input, output }
    }

    fn run(&self, output: &Path, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_webptune"))
            .current_dir(self.dir.path())
            .arg("--input")
            .arg(&self.input)
            .arg("--output")
            .arg(output)
            .args(extra)
            .output()
            .unwrap()
    }

    fn source_unchanged(&self) {
        assert_eq!(fs::read(&self.input).unwrap(), SOURCE);
    }
}

fn assert_failure(output: &Output, diagnostic: &str) {
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(diagnostic),
        "expected {diagnostic:?} in {stderr:?}"
    );
}

#[test]
fn help_and_version_work_without_required_paths() {
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        if flag == "--help" || flag == "-h" {
            for text in [
                "Usage: webptune",
                "webptune -i photo.png",
                "--input",
                "--output",
                "--batch",
                "--recursive",
                "--jobs",
                "--threshold",
                "--scale",
                "--scale-width",
                "--scale-height",
                "--search",
                "--skip-if-larger",
                "--max-bytes",
                "--quiet",
                "[default: heuristic]",
                "exhaustive",
                "--verbose",
                "[default: 80]",
                "[default: 1.0]",
                "Existing destination files are replaced",
            ] {
                assert!(stdout.contains(text), "{text:?} missing from {stdout}");
            }
        } else {
            assert_eq!(
                stdout.trim(),
                concat!("webptune ", env!("CARGO_PKG_VERSION"))
            );
        }
    }
}

#[test]
fn required_arguments_and_unknown_options_fail() {
    for args in [vec![], vec!["-i", "source.png"], vec!["-o", "out.webp"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .args(args)
            .output()
            .unwrap();
        assert_failure(&output, "required arguments");
        assert_eq!(output.status.code(), Some(2));
    }
    let fixture = Fixture::new();
    assert_failure(
        &fixture.run(&fixture.output, &["--unknown"]),
        "unexpected argument",
    );
    assert!(!fixture.output.exists());
}

#[test]
fn valid_arguments_write_webp_and_replace_existing_destinations() {
    let fixture = Fixture::new();
    for extra in [
        vec![],
        vec!["-t", "100", "-s", "0.5", "-v"],
        vec!["--threshold", "0", "--scale", "1", "--verbose"],
    ] {
        let result = fixture.run(&fixture.output, &extra);
        assert!(result.status.success(), "{:?}", result.stderr);
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Wrote WebP"));
        image::load_from_memory_with_format(
            &fs::read(&fixture.output).unwrap(),
            image::ImageFormat::WebP,
        )
        .unwrap();
    }
    // Short path flags must work at the executable boundary too.
    let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("-i")
        .arg(&fixture.input)
        .arg("-o")
        .arg(&fixture.output)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Trial "));
    fs::write(&fixture.output, DESTINATION).unwrap();
    assert!(fixture.run(&fixture.output, &[]).status.success());
    assert_ne!(fs::read(&fixture.output).unwrap(), DESTINATION);
    assert_eq!(fs::read_dir(fixture.dir.path()).unwrap().count(), 2);
    fixture.source_unchanged();
}

#[test]
fn invalid_numbers_never_write_or_truncate() {
    let fixture = Fixture::new();
    for (flag, values, diagnostic) in [
        (
            "--threshold",
            vec!["NaN", "inf", "-inf", "-1", "100.01", "nope"],
            "threshold must be a finite number in [0, 100]",
        ),
        (
            "--scale",
            vec!["NaN", "inf", "-inf", "0", "-0.5", "1.1", "nope"],
            "scale must be a finite number in (0, 1]",
        ),
    ] {
        for value in values {
            let output = fixture.run(&fixture.output, &[flag, value]);
            assert_failure(&output, diagnostic);
            assert_eq!(output.status.code(), Some(2));
            assert!(!fixture.output.exists());
        }
    }
    fs::write(&fixture.output, DESTINATION).unwrap();
    assert_failure(
        &fixture.run(&fixture.output, &["-t", "NaN"]),
        "threshold must",
    );
    assert_eq!(fs::read(&fixture.output).unwrap(), DESTINATION);
    fixture.source_unchanged();
}

#[test]
fn invalid_or_conflicting_dimension_options_preserve_existing_output() {
    let fixture = Fixture::new();
    fs::write(&fixture.output, DESTINATION).unwrap();
    for flag in ["--scale-width", "--scale-height"] {
        for value in ["0", "-1", "1.5", "NaN", "inf", "4294967296", "abc"] {
            let result = fixture.run(&fixture.output, &[flag, value]);
            assert_failure(&result, "dimension must be an integer");
            assert_eq!(result.status.code(), Some(2));
            assert_eq!(fs::read(&fixture.output).unwrap(), DESTINATION);
        }
        let result = fixture.run(&fixture.output, &[flag]);
        assert_failure(&result, "a value is required");
        assert_eq!(result.status.code(), Some(2));
        for scale in ["-s", "--scale"] {
            let result = fixture.run(&fixture.output, &[flag, "8", scale, "1"]);
            assert_failure(&result, "cannot be used with");
            assert_eq!(result.status.code(), Some(2));
            assert_eq!(fs::read(&fixture.output).unwrap(), DESTINATION);
        }
    }
    fixture.source_unchanged();
    assert_eq!(fs::read_dir(fixture.dir.path()).unwrap().count(), 2);
}

#[test]
fn invalid_search_options_preserve_existing_output_and_source() {
    let fixture = Fixture::new();
    fs::write(&fixture.output, DESTINATION).unwrap();
    for value in ["fast", "thorough", "EXHAUSTIVE", "101", ""] {
        let result = fixture.run(&fixture.output, &["--search", value]);
        assert_failure(&result, "heuristic, exhaustive");
        assert_eq!(result.status.code(), Some(2));
        assert_eq!(fs::read(&fixture.output).unwrap(), DESTINATION);
    }
    let result = fixture.run(&fixture.output, &["--search"]);
    assert_failure(&result, "a value is required");
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(fs::read(&fixture.output).unwrap(), DESTINATION);
    fixture.source_unchanged();
    assert_eq!(fs::read_dir(fixture.dir.path()).unwrap().count(), 2);
}

#[test]
fn same_file_and_canonical_aliases_are_rejected() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.dir.path().join("child")).unwrap();
    for destination in [
        fixture.input.clone(),
        PathBuf::from("./source.png"),
        PathBuf::from("child/../source.png"),
    ] {
        assert_failure(&fixture.run(&destination, &[]), "refers to the input file");
        fixture.source_unchanged();
    }
}

#[test]
fn hard_link_to_source_is_rejected() {
    let fixture = Fixture::new();
    fs::hard_link(&fixture.input, &fixture.output).unwrap();
    assert_failure(
        &fixture.run(&fixture.output, &[]),
        "refers to the input file (hard link)",
    );
    assert_eq!(fs::read(&fixture.output).unwrap(), SOURCE);
    fixture.source_unchanged();
}

#[cfg(unix)]
#[test]
fn symlink_aliases_and_dangling_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    symlink(&fixture.input, &fixture.output).unwrap();
    assert_failure(
        &fixture.run(&fixture.output, &[]),
        "refers to the input file",
    );
    let input_alias = fixture.dir.path().join("input-alias.png");
    symlink(&fixture.input, &input_alias).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .arg("-i")
        .arg(&input_alias)
        .arg("-o")
        .arg(&fixture.input)
        .output()
        .unwrap();
    assert_failure(&output, "refers to the input file");
    let dangling = fixture.dir.path().join("dangling.webp");
    let target = fixture.dir.path().join("missing.webp");
    symlink(&target, &dangling).unwrap();
    assert_failure(&fixture.run(&dangling, &[]), "cannot resolve output");
    assert!(!target.exists());
    fixture.source_unchanged();
}

#[test]
fn missing_input_directory_input_and_stdin_are_rejected() {
    let fixture = Fixture::new();
    for (input, diagnostic) in [
        (
            fixture.dir.path().join("missing.png"),
            "cannot inspect input",
        ),
        (fixture.dir.path().to_path_buf(), "must be a regular file"),
        (PathBuf::from("-"), "stdin input is not supported"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
            .arg("-i")
            .arg(input)
            .arg("-o")
            .arg(&fixture.output)
            .output()
            .unwrap();
        assert_failure(&output, diagnostic);
        assert!(!fixture.output.exists());
    }
}

#[test]
fn invalid_output_directory_does_not_create_anything() {
    let fixture = Fixture::new();
    assert_failure(
        &fixture.run(fixture.dir.path(), &[]),
        "must be a regular file",
    );
    let output = fixture.dir.path().join("missing").join("output.webp");
    assert_failure(&fixture.run(&output, &[]), "choose an existing directory");
    assert!(!fixture.dir.path().join("missing").exists());
    fixture.source_unchanged();
}

#[test]
fn stdout_target_has_no_text_payload_on_failure() {
    let fixture = Fixture::new();
    assert_failure(&fixture.run(Path::new("-"), &["-s", "0"]), "scale must");
    assert!(!fixture.dir.path().join("-").exists());
    fixture.source_unchanged();

    // A file named '-' is still accessible through an explicit path.
    fs::write(fixture.dir.path().join("-"), SOURCE).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_webptune"))
        .current_dir(fixture.dir.path())
        .args(["-i", "./-", "-o", "./destination.webp"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read(fixture.dir.path().join("-")).unwrap(), SOURCE);
    image::load_from_memory_with_format(
        &fs::read(&fixture.output).unwrap(),
        image::ImageFormat::WebP,
    )
    .unwrap();
}

//! Directory discovery, collision preflight and a bounded pool of image workers.

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use anyhow::{bail, ensure, Context, Result};

use crate::cli::{Args, OutputTarget};
use crate::output::WriteOutcome;

/// A complete, sorted mapping, validated before any directory or output is made.
/// Paths and options are retained, never decoded images or encoded candidates.
pub struct BatchPlan {
    tasks: Vec<Args>,
    output_root: PathBuf,
    workers: usize,
}

#[derive(Debug)]
pub struct BatchFailure {
    pub input: PathBuf,
    pub error: anyhow::Error,
}

#[derive(Debug, Default)]
pub struct BatchSummary {
    pub written: usize,
    pub skipped: usize,
    /// Failures in sorted source order, regardless of completion order.
    pub failures: Vec<BatchFailure>,
}

impl BatchPlan {
    /// Select .jpg/.jpeg/.png/.webp/.bmp/.hdr files, case insensitively. Do not follow
    /// directory entries that are symlinks, or read special files. The two roots
    /// must exist and have disjoint canonical trees. Generated subdirectories
    /// may be absent; existing output symlinks are refused in batch mode.
    pub fn new(args: &Args) -> Result<Self> {
        ensure!(args.batch, "directory processing requires --batch");
        let workers = args.jobs.unwrap_or(1);
        ensure!(
            (1..=64).contains(&workers),
            "jobs must be an integer in [1, 64]"
        );
        let OutputTarget::File(output) = &args.output else {
            bail!("batch output must be an existing directory; stdout is not supported");
        };
        let input_root = args.input.canonicalize().with_context(|| {
            format!(
                "cannot resolve batch input directory '{}'",
                args.input.display()
            )
        })?;
        let output_root = output.canonicalize().with_context(|| {
            format!(
                "cannot resolve batch output directory '{}'; choose an existing directory",
                output.display()
            )
        })?;
        ensure!(input_root.is_dir(), "batch input must be a directory");
        ensure!(output_root.is_dir(), "batch output must be a directory");
        ensure!(
            !input_root.starts_with(&output_root) && !output_root.starts_with(&input_root),
            "batch input and output directories must not overlap"
        );

        let mut sources = Vec::new();
        let mut directories = vec![input_root.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)
                .with_context(|| format!("cannot list input directory '{}'", directory.display()))?
            {
                let entry = entry.context("cannot read batch directory entry")?;
                let kind = entry.file_type().with_context(|| {
                    format!("cannot inspect input '{}'", entry.path().display())
                })?;
                if kind.is_file() && supported_extension(&entry.path()) {
                    sources.push(entry.path());
                } else if kind.is_dir() && args.recursive {
                    directories.push(entry.path());
                }
            }
        }
        sources.sort();
        ensure!(
            !sources.is_empty(),
            "no supported images found in batch input directory"
        );

        // Hash file identities while promptly closing handles, so large batches
        // do not exhaust the process's file-descriptor limit. Confirm any hash
        // match with actual handle equality; hash collisions cannot reject a file.
        let mut identities: HashMap<u64, Vec<&Path>> = HashMap::new();
        for source in &sources {
            identities
                .entry(identity_hash(source)?)
                .or_default()
                .push(source);
        }
        let mut files = HashSet::new();
        let mut parents = HashSet::new();
        let mut tasks = Vec::with_capacity(sources.len());
        for source in &sources {
            let relative = source.strip_prefix(&input_root)?.with_extension("webp");
            let key = portable_key(&relative);
            ensure!(
                files.insert(key.clone()),
                "batch output name collision at '{}'; use distinct image stems",
                relative.display()
            );
            ensure!(
                !parents.contains(&key),
                "batch file/directory collision at '{}'",
                relative.display()
            );
            for parent in relative
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                let key = portable_key(parent);
                ensure!(
                    !files.contains(&key),
                    "batch file/directory collision at '{}'",
                    parent.display()
                );
                parents.insert(key);
            }

            validate_destination(&output_root, &relative)?;
            let destination = output_root.join(relative);
            if destination.try_exists()? {
                if let Some(possible_sources) = identities.get(&identity_hash(&destination)?) {
                    for source in possible_sources {
                        ensure!(
                            !same_file::is_same_file(source, &destination)?,
                            "batch output '{}' refers to an input file '{}'",
                            destination.display(),
                            source.display()
                        );
                    }
                }
            }
            let mut task = args.clone();
            task.input = source.clone();
            task.output = OutputTarget::File(destination);
            tasks.push(task);
        }
        Ok(Self {
            workers: workers.min(tasks.len()),
            tasks,
            output_root,
        })
    }

    /// Run at most `--jobs` entire decode/search/write pipelines at once.
    /// Every selected image is attempted, including after another image fails.
    /// The callback must release its image/candidate buffers before returning.
    /// Successful outputs are retained; a batch is not a filesystem transaction.
    pub fn execute<F>(&self, convert: F) -> Result<BatchSummary>
    where
        F: Fn(&Args) -> Result<WriteOutcome> + Sync,
    {
        let next = AtomicUsize::new(0);
        let results = thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..self.workers {
                let next = &next;
                let convert = &convert;
                let worker = thread::Builder::new()
                    .spawn_scoped(scope, move || {
                        let mut results = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(task) = self.tasks.get(index) else {
                                break;
                            };
                            let result = self.prepare_parent(task).and_then(|()| convert(task));
                            results.push((index, result));
                        }
                        results
                    })
                    .context("cannot start batch worker")?;
                workers.push(worker);
            }
            let mut results = Vec::new();
            // Join every worker even on a panic; scoped threads must not leave
            // an unjoined panic to unwind through the coordinator.
            let mut panicked = false;
            for worker in workers {
                match worker.join() {
                    Ok(completed) => results.extend(completed),
                    Err(_) => panicked = true,
                }
            }
            ensure!(
                !panicked,
                "batch worker panicked; some outputs may have been written"
            );
            Ok::<_, anyhow::Error>(results)
        })?;
        let mut results = results;
        results.sort_by_key(|(index, _)| *index);
        let mut summary = BatchSummary::default();
        for (index, result) in results {
            match result {
                Ok(WriteOutcome::Written) => summary.written += 1,
                Ok(WriteOutcome::Skipped { .. }) => summary.skipped += 1,
                Err(error) => summary.failures.push(BatchFailure {
                    input: self.tasks[index].input.clone(),
                    error,
                }),
            }
        }
        Ok(summary)
    }

    fn prepare_parent(&self, task: &Args) -> Result<()> {
        let OutputTarget::File(destination) = &task.output else {
            unreachable!()
        };
        let relative = destination.strip_prefix(&self.output_root)?;
        // Recheck components rather than allowing create_dir_all to follow
        // preexisting symlinks. As with single-file output, this is not race-proof.
        validate_destination(&self.output_root, relative)?;
        let mut parent = self.output_root.clone();
        for component in relative.parent().into_iter().flat_map(Path::components) {
            parent.push(component);
            match fs::create_dir(&parent) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot create output directory '{}'", parent.display())
                    })
                }
            }
            let metadata = fs::symlink_metadata(&parent)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "batch output directory '{}' must be a real directory",
                parent.display()
            );
        }
        Ok(())
    }
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|extension| {
            ["jpg", "jpeg", "png", "webp", "bmp", "hdr"]
                .iter()
                .any(|e| extension.eq_ignore_ascii_case(e))
        })
}

// Conservatively reject case-folded mappings even on case-sensitive filesystems
// so common case-insensitive destinations cannot silently lose an output.
fn portable_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn identity_hash(path: &Path) -> Result<u64> {
    let handle = same_file::Handle::from_file(
        File::open(path).with_context(|| format!("cannot read batch file '{}'", path.display()))?,
    )
    .with_context(|| format!("cannot identify batch file '{}'", path.display()))?;
    let mut hasher = DefaultHasher::new();
    handle.hash(&mut hasher);
    Ok(hasher.finish())
}

fn validate_destination(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        // The final component is a file, all preceding ones are directories.
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "batch output '{}' must not be a symlink",
                    path.display()
                );
                ensure!(
                    if path == root.join(relative) {
                        metadata.is_file()
                    } else {
                        metadata.is_dir()
                    },
                    "batch output path has the wrong file type: '{}'",
                    path.display()
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot inspect batch output '{}'", path.display()))
            }
        }
    }
    Ok(())
}

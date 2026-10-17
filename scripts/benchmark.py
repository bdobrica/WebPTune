#!/usr/bin/env python3
"""Run prebuilt benchmark/CLI binaries; Python standard library only."""

import argparse
import csv
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time


def cli_measure(command):
    """wait4 returns this child's peak RSS, never a cumulative children maximum."""
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        start = time.perf_counter()
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr)
        peak_kib = ""
        if hasattr(os, "wait4"):
            _, status, usage = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)
            if sys.platform == "linux":
                peak_kib = usage.ru_maxrss
            elif sys.platform == "darwin":
                peak_kib = usage.ru_maxrss / 1024
        else:
            process.wait()
        wall_ms = (time.perf_counter() - start) * 1000
        stdout.seek(0)
        stderr.seek(0)
        diagnostic = stderr.read().decode(errors="replace")
        if process.returncode or stdout.read() or "Wrote WebP:" not in diagnostic:
            raise RuntimeError(f"CLI failed ({process.returncode}): {diagnostic}")
        return wall_ms, peak_kib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", action="append", required=True, metavar="NAME=RELEASE_DIR")
    parser.add_argument("--inputs", type=Path, default=Path("target/benchmark-inputs"))
    parser.add_argument("--output", type=Path, default=Path("target/benchmark-results"))
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--threshold", type=float, default=80)
    parser.add_argument("--scale", type=float, default=1)
    parser.add_argument("--memory-only", action="store_true",
                        help="measure fresh CLI processes without stage replay or warm-up")
    args = parser.parse_args()
    if not 1 <= args.repeats <= 100:
        parser.error("--repeats must be in 1..=100")
    inputs = sorted(args.inputs.glob("*.png"))
    if not inputs:
        parser.error("no PNG workloads; run the benchmark generate subcommand first")
    args.output.mkdir(parents=True, exist_ok=True)
    metadata = {
        "platform": platform.platform(),
        "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
        "python": platform.python_version(),
        "cpu": platform.processor(),
        "logical_cpus": os.cpu_count(),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
        "threshold": args.threshold,
        "scale": args.scale,
        "repeats": args.repeats,
        "memory_only": args.memory_only,
        "inputs_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
        "builds": {},
    }
    if sys.platform == "linux":
        metadata["cpu"] = next(
            (line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
             if line.startswith("model name")), metadata["cpu"])
    suffix = ".exe" if os.name == "nt" else ""
    with tempfile.TemporaryDirectory() as staging:
        for build in args.build:
            name, directory = build.split("=", 1)
            if not name or Path(name).name != name or name in {".", ".."}:
                parser.error("build name must be a single filename component")
            directory = Path(directory).resolve()
            example = directory / "examples" / f"benchmark{suffix}"
            binary = directory / f"webptune{suffix}"
            executables = [binary] if args.memory_only else [example, binary]
            metadata["builds"][name] = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in executables}
            with (args.output / f"{name}.csv").open("w", newline="") as output:
                writer = None
                for source in inputs:
                    print(f"{name}: {source.name}", flush=True)
                    if args.memory_only:
                        rows = [{"repeat": repeat} for repeat in range(1, args.repeats + 1)]
                    else:
                        measured = subprocess.check_output([
                            str(example), "measure", "--input", str(source),
                            "--repeats", str(args.repeats), "--threshold", str(args.threshold),
                            "--scale", str(args.scale),
                        ], text=True)
                        rows = list(csv.DictReader(io.StringIO(measured)))
                    for row in rows:
                        destination = Path(staging) / "result.webp"
                        destination.write_bytes(b"existing destination")
                        wall_ms, peak_kib = cli_measure([
                            str(binary), "-i", str(source), "-o", str(destination),
                            "-t", str(args.threshold),
                            "-s", str(args.scale),
                        ])
                        payload = destination.read_bytes()
                        if (payload[:4] != b"RIFF" or payload[8:12] != b"WEBP"
                                or (not args.memory_only and len(payload) != int(row["bytes"]))):
                            raise RuntimeError("CLI output differs from measured search size/format")
                        row["bytes"] = len(payload)
                        row = {"case": source.stem, **row, "cli_ms": f"{wall_ms:.3f}", "cli_peak_kib": peak_kib}
                        if writer is None:
                            writer = csv.DictWriter(output, fieldnames=list(row))
                            writer.writeheader()
                        writer.writerow(row)
                        output.flush()
                    if hashlib.sha256(source.read_bytes()).hexdigest() != metadata["inputs_sha256"][source.name]:
                        raise RuntimeError("benchmark source changed")
    (args.output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()

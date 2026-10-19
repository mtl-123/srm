#!/usr/bin/env python3
"""Single-host elapsed-time comparison; not a physical-delete or network benchmark."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

baseline, candidate, output = map(Path, sys.argv[1:4])
root = Path(tempfile.mkdtemp(prefix="srm-benchmark-", dir=Path.home()))
rows = []


def measure(version, binary, scenario, count=0, strategy="checked"):
    case = root / f"{version}-{scenario}"
    case.mkdir()
    store = case / "trash"
    config = case / "config.json"
    config.write_text(json.dumps({"local_store": str(store), "directory_strategy": strategy}))
    config.chmod(0o600)
    data = case / "data"
    data.mkdir()
    if scenario.startswith("directory"):
        for i in range(count):
            (data / str(i)).write_bytes(b"x" * 64)
        operands = ["-rf", str(data)]
    elif scenario == "individual-5000":
        for i in range(count):
            (data / str(i)).write_bytes(b"x" * 64)
        operands = [str(data / str(i)) for i in range(count)]
    else:
        p = data / "large"
        with p.open("wb") as f:
            os.posix_fallocate(f.fileno(), 0, 1024**3)
            f.write(b"allocated file, same-filesystem rename\n")
            f.flush()
            os.fsync(f.fileno())
        allocated = p.stat().st_blocks * 512
        assert allocated >= 1024**3
        operands = [str(p)]
    args = [str(binary)]
    if version != "v2.0":
        args += ["--config", str(config)]
    args += ["--rm", *operands]
    started = time.perf_counter()
    result = subprocess.run(args, env={**os.environ, "SRM_HOME": str(store)}, capture_output=True)
    elapsed = time.perf_counter() - started
    assert result.returncode == 0, result.stderr
    row = {"version": version, "scenario": scenario, "count": count, "directory_strategy": strategy, "seconds": round(elapsed, 6)}
    rows.append(row)
    print(json.dumps(row), flush=True)


try:
    for version, binary in (("v2.0", baseline), ("v2.1", candidate)):
        measure(version, binary, "individual-5000", 5000)
        measure(version, binary, "directory-50000", 50000)
        measure(version, binary, "allocated-1GiB")
    measure("v2.1-fast", candidate, "directory-50000", 50000, "rename")
    output.write_text(json.dumps({"environment": "WSL Debian, Linux ext4, single run per scenario; creation excluded",
        "note": "Recovery moves, not physical erasure. No remote throughput claim.", "results": rows}, indent=2) + "\n")
finally:
    # This exact fresh temporary tree contains only this benchmark's fixtures.
    assert root.parent == Path.home() and root.name.startswith("srm-benchmark-")
    shutil.rmtree(root)

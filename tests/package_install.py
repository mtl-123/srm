#!/usr/bin/env python3
"""Smoke-test the final trusted build archive in a fresh user environment."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(tempfile.mkdtemp(prefix="srm-package-test-", dir=Path.home()))
subprocess.run(["tar", "-xzf", str(Path(sys.argv[1]).resolve()), "-C", str(root)], check=True)
project = next(p for p in root.iterdir() if p.is_dir())
home = root / "home"
home.mkdir()
env = {k: v for k, v in os.environ.items() if k not in ("HOME", "SRM_HOME", "SRM_CONFIG", "SRM_LANG", "XDG_CONFIG_HOME", "XDG_DATA_HOME")}
env["HOME"] = str(home)
for _ in range(2):
    subprocess.run(["bash", str(project / "scripts/install.sh"), "--binary", str(project / "srm")], env=env, check=True)
binary = home / ".local/bin/srm"
assert not (home / ".local/bin/rm").exists()
store = home / ".local/share/safe-srm"
assert store.stat().st_mode & 0o777 == 0o700
source = home / "payload"
source.write_text("fresh package restore")
subprocess.run([str(binary), "--rm", str(source)], env=env, check=True)
records = json.loads(subprocess.check_output([str(binary), "ls", "--json"], env=env))
subprocess.run([str(binary), "res", records[0]["id"]], env=env, check=True)
assert source.read_text() == "fresh package restore"
print("Final archive: repeat user installation, default store, delete/restore, native rm isolation passed")

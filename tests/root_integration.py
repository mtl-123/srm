#!/usr/bin/env python3
"""Root/sudo isolation tests on new root-owned fixtures only."""
import json
import os
from pathlib import Path
import pwd
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
user = pwd.getpwnam(sys.argv[2])
root = Path(tempfile.mkdtemp(prefix="srm-root-test-", dir="/run"))
config = root / "config.json"
config.write_text(json.dumps({"local_store": str(root / "store")}))
config.chmod(0o600)
source = root / "root-owned"
source.write_bytes(b"root private payload")
source.chmod(0o600)
spoof = Path(user.pw_dir) / ("must-not-create-" + root.name)
env = {**os.environ, "HOME": user.pw_dir, "SRM_HOME": str(spoof), "XDG_DATA_HOME": user.pw_dir,
       "SRM_CONFIG": str(spoof / "untrusted.json")}


def call(*args, check=True):
    p = subprocess.run(["sudo", "-n", "--", binary, "--config", str(config), *map(str, args)],
                       env=env, capture_output=True, text=True, timeout=30)
    if check:
        assert p.returncode == 0, (p.stdout, p.stderr)
    return p


denied = subprocess.run([binary, "--rm", str(source)], user=user.pw_uid, group=user.pw_gid, extra_groups=[],
                        env={**os.environ, "HOME": user.pw_dir}, capture_output=True)
assert denied.returncode != 0 and source.exists()
call("--rm", source)
assert not source.exists()
rows = json.loads(call("list", "--json").stdout)
assert len(rows) == 1
assert Path(rows[0]["trash_path"]).stat().st_uid == 0
call("restore", rows[0]["id"])
assert source.read_bytes() == b"root private payload"
assert source.stat().st_uid == 0 and source.stat().st_mode & 0o777 == 0o600
assert not spoof.exists()
call("--lang", "zh")
saved = json.loads(config.read_text())
assert saved["language"] == "zh" and saved["local_store"] == str(root / "store")
assert not spoof.exists()
call("--lang", "en")
assert json.loads(config.read_text())["language"] == "en"
print("Root language preference persists only in the trusted root config: passed")
print("sudo removes/restores root-owned file, preserves 0600/uid 0, isolates inherited environment: passed")
bad = Path(tempfile.mkdtemp(prefix="srm-root-untrusted-", dir=user.pw_dir))
os.chown(bad, user.pw_uid, user.pw_gid)
unsafe_config = bad / "root-owned-but-user-parent.json"
unsafe_config.write_text(json.dumps({"local_store": str(root / "unsafe-store")}))
unsafe_config.chmod(0o600)
result = subprocess.run([binary, "--config", str(unsafe_config), "--rm", str(source)], capture_output=True)
assert result.returncode != 0 and source.exists()
print("root refuses config below a user-owned ancestor: passed")
print("Root test evidence:", root)

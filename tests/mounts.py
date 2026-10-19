#!/usr/bin/env python3
"""Run ONLY in a private mount namespace as root; tested commands run unprivileged.
Example: unshare --mount --propagation private python3 tests/mounts.py BINARY USER
"""
import hashlib
import json
import os
from pathlib import Path
import pwd
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
user = pwd.getpwnam(sys.argv[2])
root = Path(tempfile.mkdtemp(prefix="srm-mount-tests-"))
os.chown(root, user.pw_uid, user.pw_gid)
mounted = []


def directory(path):
    path.mkdir(mode=0o700)
    os.chown(path, user.pw_uid, user.pw_gid)
    return path


def payload(path, content=b"payload"):
    path.write_bytes(content)
    os.chown(path, user.pw_uid, user.pw_gid)


def mount(path, size):
    subprocess.run(["mount", "-t", "tmpfs", "-o", f"size={size},uid={user.pw_uid},gid={user.pw_gid},mode=0700", "srm-test", str(path)], check=True)
    mounted.append(path)


def call(args, trash):
    return subprocess.run(args, env={**os.environ, "SRM_HOME": str(trash), "HOME": user.pw_dir},
                          user=user.pw_uid, group=user.pw_gid, extra_groups=[],
                          capture_output=True, text=True, timeout=30)


try:
    # A mounted subtree must never be silently moved into the trash.
    for label, command in (("gnu", ["/bin/rm"]), ("srm", [binary, "--rm"])):
        case = directory(root / label)
        tree = directory(case / "tree")
        point = directory(tree / "mount")
        mount(point, "4m")
        payload(point / "keep")
        payload(tree / "remove")
        result = call([*command, "-rf", "--one-file-system", str(tree)], case / "trash")
        assert result.returncode != 0, result
        assert (point / "keep").read_bytes() == b"payload"
        assert not (tree / "remove").exists()
        print(label, "--one-file-system: passed")
        result = call([*command, "-rf", "--preserve-root=all", str(point)], case / "trash")
        assert result.returncode != 0
        assert (point / "keep").exists()
        print(label, "--preserve-root=all: passed")

    for label, command in (("gnu-bind", ["/bin/rm"]), ("srm-bind", [binary, "--rm"])):
        case = directory(root / label)
        tree = directory(case / "tree")
        original = directory(case / "original")
        point = directory(tree / "mount")
        payload(original / "remove")
        subprocess.run(["mount", "--bind", str(original), str(point)], check=True)
        mounted.append(point)
        result = call([*command, "-rf", str(tree)], case / "trash")
        assert result.returncode != 0
        assert point.is_dir()
        assert not (original / "remove").exists()
        if label.startswith("srm"):
            rows = json.loads(call([binary, "list", "--json"], case / "trash").stdout)
            assert len(rows) == 1 and rows[0]["state"] == "trashed"
        print(label, "same-device bind mount contents and mount boundary: passed")

    case = directory(root / "enospc")
    trash = directory(case / "trash")
    mount(trash, "1m")
    source = case / "source"
    payload(source, b"abcdefgh" * (512 * 1024))
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    result = call([binary, "--rm", str(source)], trash)
    assert result.returncode != 0, result
    assert hashlib.sha256(source.read_bytes()).hexdigest() == digest
    rows = json.loads(call([binary, "list", "--json"], trash).stdout)
    assert len(rows) == 1 and rows[0]["state"] == "copying"
    assert call([binary, "doctor"], trash).returncode != 0
    print("cross-device ENOSPC preserves source and journal: passed")
finally:
    for path in reversed(mounted):
        subprocess.run(["umount", str(path)], check=True)
print("Private namespace test evidence:", root)

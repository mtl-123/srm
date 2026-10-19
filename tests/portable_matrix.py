#!/usr/bin/env python3
"""Run portable binary smoke checks in isolated distribution containers."""
import json
from pathlib import Path
import subprocess
import sys

binary = str(Path(sys.argv[1]).resolve())
output = Path(sys.argv[2]).resolve()
images = sys.argv[3:] or ["debian:12-slim", "ubuntu:22.04", "ubuntu:24.04", "rockylinux:9", "fedora:42", "opensuse/leap:15.6", "alpine:3.20"]
script = r'''
set -eu
mkdir -p /run/srm-check
chmod 700 /run/srm-check
printf '%s\n' '{"local_store":"/run/srm-check/trash"}' > /run/srm-check/config.json
chmod 600 /run/srm-check/config.json
srm() { /opt/srm --config /run/srm-check/config.json "$@"; }
cat /etc/os-release
/opt/srm --version
mkdir -p /run/srm-check/data/tree/sub
printf 'recover me\n' > /run/srm-check/data/tree/sub/file
ln -s missing /run/srm-check/data/tree/dangling
srm --rm -rf /run/srm-check/data/tree
test ! -e /run/srm-check/data/tree
id=$(srm list | cut -f1)
srm restore "$id"
test "$(cat /run/srm-check/data/tree/sub/file)" = 'recover me'
test -L /run/srm-check/data/tree/dangling
srm del -d 0 /run/srm-check/data/tree
srm clean
test -z "$(srm list)"
srm doctor
printf 'PORTABLE_SMOKE_OK\n'
'''
results = []
for image in images:
    try:
        p = subprocess.run(["docker", "run", "--rm", "--network=none", "--read-only", "--tmpfs", "/run:rw,nosuid,nodev,mode=755",
                            "--mount", f"type=bind,src={binary},dst=/opt/srm,readonly",
                            image, "sh", "-c", script], capture_output=True, text=True, timeout=150)
        row = {"image": image, "passed": p.returncode == 0 and "PORTABLE_SMOKE_OK" in p.stdout,
               "exit_code": p.returncode, "stdout": p.stdout, "stderr": p.stderr}
    except subprocess.TimeoutExpired:
        row = {"image": image, "passed": False, "error": "container execution or image retrieval timed out"}
    results.append(row)
    output.write_text(json.dumps(results, indent=2) + "\n")
    print(image, "PASS" if row["passed"] else "UNVERIFIED/FAIL", flush=True)

#!/usr/bin/env python3
"""Real SSH tests on a temporary loopback-only sshd; no production accounts/keys changed.
Run as root: python3 tests/remote_integration.py BINARY USER EXTRACTED_OPENSSH_ROOT
"""
import json
import os
from pathlib import Path
import pwd
import shutil
import socket
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
user = pwd.getpwnam(sys.argv[2])
sshd_root = Path(sys.argv[3]).resolve() if len(sys.argv) > 3 else Path("/")
control = Path(tempfile.mkdtemp(prefix="srm-ssh-test-", dir="/run"))
control.chmod(0o755)
root = Path(tempfile.mkdtemp(prefix="srm-remote-test-", dir=user.pw_dir))
os.chown(root, user.pw_uid, user.pw_gid)
key = control / "identity"
host = control / "host"
for path in (key, host):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(path)], check=True)
os.chown(key, user.pw_uid, user.pw_gid)
key.chmod(0o600)
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
known = control / "known_hosts"
public = host.with_suffix(".pub").read_text().split()
known.write_text(f"[127.0.0.1]:{port} {public[0]} {public[1]}\n")
known.chmod(0o644)
config = control / "sshd_config"
config.write_text(f"""Port {port}
ListenAddress 127.0.0.1
HostKey {host}
PidFile {control}/sshd.pid
AuthorizedKeysFile {key}.pub
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM yes
AllowUsers {user.pw_name}
StrictModes yes
LogLevel ERROR
""")
if sshd_root != Path("/"):
    with config.open("a") as f:
        f.write(f"SshdSessionPath {sshd_root}/usr/lib/openssh/sshd-session\nSshdAuthPath {sshd_root}/usr/lib/openssh/sshd-auth\n")
server_env = dict(os.environ)
if sshd_root != Path("/"):
    server_env["LD_LIBRARY_PATH"] = str(next(sshd_root.rglob("libwrap.so.0")).parent)
Path("/run/sshd").mkdir(mode=0o755, exist_ok=True)
server = subprocess.Popen([str(sshd_root / "usr/sbin/sshd"), "-D", "-e", "-f", str(config)],
                          env=server_env,
                          stdout=(control / "sshd.log").open("w"), stderr=subprocess.STDOUT)


def owned(path, directory=False):
    if directory:
        path.mkdir(mode=0o700)
    os.chown(path, user.pw_uid, user.pw_gid)
    return path


def call(cfg, *args, check=True):
    p = subprocess.run([binary, "--config", str(cfg), *map(str, args)],
                       user=user.pw_uid, group=user.pw_gid, extra_groups=[],
                       env={**os.environ, "HOME": user.pw_dir}, capture_output=True, text=True, timeout=40)
    if check:
        assert p.returncode == 0, (args, p.stdout, p.stderr, (control / "sshd.log").read_text()[-1500:])
    return p


def setup(name, backend="remote_async", release=True, bad_port=False):
    case = owned(root / name, True)
    cfg = case / "config.json"
    cfg.write_text(json.dumps({"backend": backend, "local_store": str(case / "store"), "expire_days": 7, "batch_size": 256,
        "remote": {"host": "127.0.0.1", "port": 1 if bad_port else port, "user": user.pw_name,
                   "directory": str(root / "remote-storage"), "namespace": name,
                   "identity_file": str(key), "known_hosts_file": str(known),
                   "release_local_after_upload": release, "timeout_seconds": 20}}))
    owned(cfg).chmod(0o600)
    return case, cfg


def records(cfg):
    return json.loads(call(cfg, "list", "--json").stdout)


try:
    for _ in range(100):
        if server.poll() is not None:
            raise RuntimeError((control / "sshd.log").read_text())
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            time.sleep(0.05)
    case, cfg = setup("roundtrip")
    call(cfg, "remote", "init")
    tree = owned(case / "tree", True)
    for i in range(100):
        path = tree / f"small-{i}"
        path.write_text(f"payload {i}")
        owned(path)
    owned(tree / "nested", True)
    (tree / "nested/link").symlink_to("../small-0")
    os.lchown(tree / "nested/link", user.pw_uid, user.pw_gid)
    os.link(tree / "small-0", tree / "hardlink")
    call(cfg, "--rm", "-rf", tree)
    assert not tree.exists()
    # Both the automatically launched worker and an explicit sync are idempotent.
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        rows = records(cfg)
        if rows[0]["state"] == "remote_only":
            break
        time.sleep(0.1)
    else:
        raise RuntimeError((case / "store/upload.log").read_text())
    item = rows[0]
    assert not Path(item["trash_path"]).exists()
    call(cfg, "restore", item["id"])
    assert (tree / "small-99").read_text() == "payload 99"
    assert (tree / "nested/link").is_symlink()
    assert (tree / "hardlink").stat().st_ino == (tree / "small-0").stat().st_ino
    print("real SSH async archive, acknowledgement, eviction, restoration: passed", flush=True)
    remote = json.loads(call(cfg, "remote", "list").stdout)
    destination = case / "disaster-recovery"
    call(cfg, "remote", "restore", remote[0]["object"], "-t", destination)
    assert (destination / "small-99").read_text() == "payload 99"
    print("remote-only catalog recovery with no local record: passed", flush=True)

    case, cfg = setup("offline", bad_port=True)
    path = case / "source"
    path.write_text("never lose this")
    owned(path)
    started = time.monotonic()
    call(cfg, "--rm", path)
    elapsed = time.monotonic() - started
    assert elapsed < 3, elapsed
    assert not path.exists()
    rows = records(cfg)
    assert Path(rows[0]["trash_path"]).read_text() == "never lose this"
    assert call(cfg, "empty", "-y", check=False).returncode != 0
    print("offline async fast return and cleanup protection: passed", flush=True)

    case, cfg = setup("sync-offline", backend="remote_sync", bad_port=True)
    path = case / "source"
    path.write_text("must remain at source")
    owned(path)
    assert call(cfg, "--rm", path, check=False).returncode != 0
    assert path.read_text() == "must remain at source"
    print("synchronous failure retains original pathname: passed", flush=True)

    case, cfg = setup("checksum", backend="remote_sync")
    path = case / "source"
    path.write_text("verify me")
    owned(path)
    call(cfg, "--rm", path)
    item = records(cfg)[0]
    archive = root / "remote-storage/checksum" / (item["remote"]["object"] + ".tar")
    archive.write_bytes(b"tampered")
    assert call(cfg, "restore", item["id"], check=False).returncode != 0
    assert not path.exists()
    assert len(records(cfg)) == 1
    print("tampered remote bytes rejected before extraction: passed", flush=True)

    root_config = control / "root-config.json"
    root_key = control / "root-identity"
    shutil.copyfile(key, root_key)
    root_key.chmod(0o600)
    root_config.write_text(json.dumps({"backend": "remote_sync", "local_store": str(control / "root-store"),
        "remote": {"host": "127.0.0.1", "port": port, "user": user.pw_name, "directory": str(root / "remote-storage"),
                   "namespace": "root-test", "identity_file": str(root_key), "known_hosts_file": str(known),
                   "release_local_after_upload": True, "timeout_seconds": 20}}))
    root_config.chmod(0o600)
    root_file = control / "root-owned"
    root_file.write_text("root data")
    root_file.chmod(0o600)
    def root_call(*args):
        return subprocess.run([binary, "--config", str(root_config), *map(str,args)], capture_output=True, text=True, check=True)
    root_call("--rm", root_file)
    row = json.loads(root_call("list", "--json").stdout)[0]
    root_call("restore", row["id"])
    assert root_file.read_text() == "root data" and root_file.stat().st_uid == 0 and root_file.stat().st_mode & 0o777 == 0o600
    print("root remote archive/restoration preserves uid 0 and mode 0600: passed", flush=True)
finally:
    server.terminate()
    server.wait(timeout=10)
print("SSH test artifacts:", root)
print("Temporary server stopped; production SSH configuration was not changed.")

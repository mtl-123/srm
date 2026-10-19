"""SSH protocol helper, embedded by srm. Requires Python 3 on the destination.
Archives are immutable objects. Only a hash-verified, fsynced object is committed.
No extraction or shell execution occurs on the storage machine.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile


def private_dir(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    st = path.lstat()
    if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_mode & 0o077:
        raise RuntimeError("remote directory must be private, owned by the SSH user, and not a symlink")


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def read_json(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as f:
        return json.load(f)


def main():
    if sys.argv[1] == "validate-local":
        import tarfile
        archive, expected = sys.argv[2:4]
        links = set()
        paths = []
        with tarfile.open(archive, "r:") as stream:
            for member in stream:
                parts = member.name.rstrip("/").split("/")
                if not parts or parts[0] != expected or any(p in ("", ".", "..") for p in parts):
                    raise RuntimeError("unsafe archive member path")
                if member.islnk():
                    target = member.linkname.split("/")
                    if target[0] != expected or any(p in ("", ".", "..") for p in target):
                        raise RuntimeError("unsafe archive hardlink")
                if member.issym() or member.islnk():
                    links.add(member.name.rstrip("/"))
                paths.append(parts)
        if any("/".join(parts[:n]) in links for parts in paths for n in range(1, len(parts))):
            raise RuntimeError("archive contains entries below a link")
        return
    action, base, namespace = sys.argv[1:4]
    if not os.path.isabs(base) or not re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9_.-]*", namespace) or namespace in (".", ".."):
        raise RuntimeError("invalid destination")
    root = Path(base)
    private_dir(root)
    root = root / namespace
    private_dir(root)
    if action == "init":
        print(json.dumps({"ready": True, "directory": str(root)}))
        return
    if action == "list":
        print(json.dumps([read_json(p) for p in sorted(root.glob("*.json"))]))
        return
    key = sys.argv[4]
    if not re.fullmatch(r"[fdl]_[a-f0-9]{16}-[a-f0-9]{16}", key):
        raise RuntimeError("invalid object key")
    archive = root / (key + ".tar")
    manifest = root / (key + ".json")
    pending = root / (key + ".pending")
    if action == "inspect":
        print(json.dumps(read_json(manifest)))
    elif action == "send":
        read_json(manifest)  # Only committed objects can be fetched.
        fd = os.open(archive, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "rb") as f:
            while True:
                data = f.read(1024 * 1024)
                if not data:
                    break
                sys.stdout.buffer.write(data)
    elif action == "commit":
        result = read_json(manifest if manifest.exists() else pending)
        if result["sha256"] != sys.argv[5]:
            raise RuntimeError("digest mismatch; object not committed")
        if not manifest.exists():
            os.link(pending, manifest)
            pending.unlink()
            sync_dir(root)
        print(json.dumps(result))
    elif action == "delete":
        # A deletion journal is kept by the client; retry after a broken connection.
        leftovers = list(root.glob(key + ".*.part")) + list(root.glob(key + ".*.pending"))
        for path in (manifest, archive, pending, *leftovers):
            try:
                path.unlink()
            except FileNotFoundError:
                pass
        sync_dir(root)
        print(json.dumps({"deleted": key}))
    elif action == "receive":
        if archive.exists() or manifest.exists():
            raise RuntimeError("object already exists; use a fresh object key")
        header = sys.stdin.buffer.readline(1024 * 1024)
        if not header.endswith(b"\n"):
            raise RuntimeError("missing metadata header")
        metadata = json.loads(header)
        fd, temporary = tempfile.mkstemp(prefix=key + ".", suffix=".part", dir=root)
        digest = hashlib.sha256()
        size = 0
        try:
            with os.fdopen(fd, "wb") as f:
                while True:
                    data = sys.stdin.buffer.read(1024 * 1024)
                    if not data:
                        break
                    digest.update(data)
                    size += len(data)
                    f.write(data)
                f.flush()
                os.fsync(f.fileno())
            # The client compares this digest with the bytes it sent before eviction.
            result = {"object": key, "sha256": digest.hexdigest(), "bytes": size, "record": metadata}
            os.link(temporary, archive)  # Atomic publish without overwriting another object.
            os.unlink(temporary)
            fd, temp_meta = tempfile.mkstemp(prefix=key + ".", suffix=".pending", dir=root)
            with os.fdopen(fd, "w") as f:
                json.dump(result, f)
                f.flush()
                os.fsync(f.fileno())
            os.link(temp_meta, pending)
            os.unlink(temp_meta)
            sync_dir(root)
            print(json.dumps(result), flush=True)
        except BaseException:
            # Partial archives are never listed as committed. Retained for inspection.
            raise
    else:
        raise RuntimeError("unsupported operation")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("srm remote: " + str(error), file=sys.stderr)
        sys.exit(1)

#!/usr/bin/env python3
"""Bounded, read-only ID suggestions. Never invokes srm, sudo, SSH or a lock."""
import json
import os
from pathlib import Path
import re
import stat
import sys
import time


def read(path, private=False):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        st = os.fstat(stream.fileno())
        if not stat.S_ISREG(st.st_mode) or st.st_uid != os.geteuid() or st.st_size > 2 * 1024 * 1024:
            return None
        if st.st_mode & (0o077 if private else 0o022):
            return None
        return json.load(stream)


def main():
    style, explicit, prefix = sys.argv[1:4]
    root = os.geteuid() == 0
    home = Path.home()
    config = Path(explicit) if explicit else (
        Path("/etc/safe-srm/config.json") if root else
        Path(os.environ.get("SRM_CONFIG", str(Path(os.environ.get("XDG_CONFIG_HOME", str(home / ".config"))) / "safe-srm/config.json"))))
    if not config.is_absolute():
        return
    settings = read(config) if config.exists() else {}
    if not isinstance(settings, dict):
        return
    if style == "language":
        language = settings.get("language", "auto")
        print(language if language in ("auto", "zh", "en") else "auto")
        return
    base = settings.get("local_store")
    if base is None:
        if root:
            base = "/var/lib/safe-srm/root"
        else:
            xdg = Path(os.environ.get("XDG_DATA_HOME", str(home / ".local/share")))
            if not xdg.is_absolute():
                xdg = home / ".local/share"
            base = os.environ.get("SRM_HOME", str(xdg / "safe-srm"))
    base = Path(base)
    if not base.is_absolute():
        return
    meta = base / "meta"
    for path in (base, meta):
        st = path.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.geteuid() or st.st_mode & 0o077:
            return
    # On root shells, reject user-writable ancestors as the executable does.
    if root:
        for path in (base, config):
            for ancestor in (path, *path.parents):
                st = ancestor.lstat()
                if stat.S_ISLNK(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022:
                    return
    deadline = time.monotonic() + 0.15
    count = 0
    with os.scandir(meta) as entries:
        for index, entry in enumerate(entries):
            if index >= 1000 or count >= 100 or time.monotonic() > deadline:
                break
            if not entry.name.endswith(".json"):
                continue
            try:
                value = read(entry.path, private=True)
                rows = value if isinstance(value, list) else [value]
                for row in rows:
                    if not isinstance(row, dict):
                        continue
                    key = row.get("id", "")
                    if not isinstance(key, str) or not re.fullmatch(r"[fdl]_[0-9a-f]{16}", key) or not key.startswith(prefix):
                        continue
                    if style == "zsh":
                        text = str(row.get("original_path", ""))
                        text = "".join(c if c.isprintable() else " " for c in text)[:160]
                        text = text.replace("\\", "\\\\").replace(":", "\\:")
                        print(key + ":" + text)
                    else:
                        print(key)
                    count += 1
                    if count >= 100:
                        break
            except (OSError, ValueError, TypeError):
                continue


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, TypeError):
        pass

#!/usr/bin/env python3
"""Require the pushed tag to match the Cargo package being released."""
import os
from pathlib import Path
import re
import tomllib

manifest = tomllib.loads((Path(__file__).resolve().parents[1] / "Cargo.toml").read_text())
version = manifest["package"]["version"]
tag = os.environ["RELEASE_TAG"]
if not re.fullmatch(r"v\d+\.\d+\.\d+", tag) or tag != "v" + version:
    raise SystemExit(f"Tag {tag!r} must equal v{version} from Cargo.toml")
print("Validated release tag:", tag)

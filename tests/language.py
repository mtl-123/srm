#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
root = Path(tempfile.mkdtemp(prefix="srm-language-", dir=Path.home()))
base = {k:v for k,v in os.environ.items() if k not in ("LC_ALL", "LC_CTYPE", "LC_MESSAGES", "LANG", "LANGUAGE", "SRM_LANG")}
base["SRM_HOME"] = str(root / "trash")
base["SRM_CONFIG"] = str(root / "config.json")

def call(args, env=None, ok=True):
    p = subprocess.run([binary, *args], env={**base, **(env or {})}, cwd=root, capture_output=True, text=True)
    if ok: assert p.returncode == 0, p.stderr
    return p

cases = [
    ({"LANG":"zh_CN.UTF-8"}, [], "zh"),
    ({"LANG":"en_US.UTF-8"}, [], "en"),
    ({"LANG":"C"}, ["--lang", "zh"], "en"),
    ({"LANG":"zh_CN.GBK"}, [], "en"),
    ({"LANG":"C.UTF-8"}, ["--lang=zh"], "zh"),
    ({"LANG":"zh_CN.UTF-8", "LC_ALL":"C"}, [], "en"),
    ({"LANG":"zh_CN.UTF-8", "LC_CTYPE":"C"}, [], "en"),
    ({"LANG":"en_US.UTF-8", "LANGUAGE":"zh_CN:en"}, [], "zh"),
    ({"LANG":"zh_CN.UTF-8", "SRM_LANG":"en"}, [], "en"),
    ({"LANG":"C.UTF-8", "SRM_LANG":"en"}, ["--lang","zh"], "zh"),
    ({}, [], "en"),
]
for env, args, language in cases:
    result = json.loads(call([*args, "--language-info"], env).stdout)
    assert result["selected"] == language, result
commands = [[], ["list"], ["restore"], ["clean"], ["empty"], ["doctor"], ["recover"], ["config"], ["remote"],
            ["remote", "init"], ["remote", "sync"], ["remote", "restore"], ["remote", "delete"], ["del"], ["--rm"]]
for command in commands:
    zh = call(["--lang", "zh", *command, "--help"], {"LANG":"C.UTF-8"}).stdout
    en = call(["--lang", "en", *command, "--help"], {"LANG":"C.UTF-8"}).stdout
    assert "用法" in zh, zh
    assert "Usage" in en and "用法" not in en, en
assert "按 ID" in call(["--lang", "zh", "help", "restore"], {"LANG":"C.UTF-8"}).stdout
for locale in ("zh_CN.UTF-8", "en_US.UTF-8"):
    help_text = call(["--help"], {"LANG": locale}).stdout
    for alias in ("ls", "res", "cln"):
        assert f"[aliases: {alias}]" in help_text, help_text
assert not (root / "trash").exists(), "help must not initialize storage"
assert call(["--lang", "bad", "--help"], ok=False).returncode != 0
assert not (root / "config.json").exists(), "temporary help must not save preferences"
call(["--lang", "zh"], {"LANG":"C.UTF-8"})
settings_path = root / "config.json"
settings = json.loads(settings_path.read_text())
assert settings["language"] == "zh"
assert "用法" in call(["--help"], {"LANG":"en_US.UTF-8"}).stdout
assert "Usage" in call(["--lang", "en", "--help"], {"LANG":"C.UTF-8"}).stdout
assert json.loads(settings_path.read_text())["language"] == "zh", "temporary override changed config"
settings["expire_days"] = 42
settings["volume_stores"] = [str(root / "other-volume")]
settings_path.write_text(json.dumps(settings))
call(["--lang", "en"], {"LANG":"zh_CN.UTF-8"})
updated = json.loads(settings_path.read_text())
assert updated == {**settings, "language":"en"}, "other settings changed"
assert "Usage" in call(["--help"], {"LANG":"zh_CN.UTF-8"}).stdout
assert settings_path.stat().st_mode & 0o777 == 0o600
call(["--lang", "zh"], {"LANG":"C"})
assert json.loads(settings_path.read_text())["language"] == "zh"
assert json.loads(call(["--language-info"], {"LANG":"C"}).stdout)["selected"] == "en"
assert json.loads(call(["--language-info"], {"LANG":"C.UTF-8"}).stdout)["selected"] == "zh"
call(["--lang", "auto"], {"LANG":"C.UTF-8"})
assert json.loads(call(["--language-info"], {"LANG":"en_US.UTF-8"}).stdout)["selected"] == "en"
before = settings_path.read_bytes()
assert call(["--lang", "invalid"], ok=False).returncode != 0
assert settings_path.read_bytes() == before
settings_path.write_text("invalid json")
assert call(["--lang", "en"], ok=False).returncode != 0
assert settings_path.read_text() == "invalid json"
settings_path.write_bytes(before)
print("Persistent language: creation, cross-process defaults, overrides, preservation, fallback and invalid-config protection passed")
filename = root / "--lang=zh"
filename.write_text("operand not option")
call(["--", "--lang=zh"], {"LANG":"C.UTF-8"})
rows = json.loads(call(["--lang", "zh", "list", "--json"], {"LANG":"C.UTF-8"}).stdout)
assert rows[0]["original_path"] == str(filename)
call(["restore", rows[0]["id"]], {"LANG":"C.UTF-8"})
assert filename.read_text() == "operand not option"
print("Language: 11 locale cases, 30 help pages, help alias, no-side-effects, invalid language, literal filename and JSON stability passed")

#!/usr/bin/env python3
"""Exercise completion dispatch without invoking deletion or network commands."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

project = Path(__file__).resolve().parents[1]
root = Path(tempfile.mkdtemp(prefix="srm-completion-", dir=Path.home()))
store = root / "store"
meta = store / "meta"
meta.mkdir(parents=True)
store.chmod(0o700)
meta.chmod(0o700)
record = meta / "f_0123456789abcdef.json"
record.write_text(json.dumps({"id": "f_0123456789abcdef", "original_path": "/data/two words:中文.txt"}))
record.chmod(0o600)
env = {**os.environ, "SRM_HOME": str(store), "_SRM_COMPLETION_HELPER": str(project / "completions/completion_ids.py")}
(root / "two words.txt").write_text("unchanged")


def bash(words):
    command = 'source "$1"; shift; COMP_WORDS=("$@"); COMP_CWORD=$((${#COMP_WORDS[@]}-1)); _srm_complete; printf "%s\\n" "${COMPREPLY[@]}"'
    result = subprocess.run(["bash", "--noprofile", "--norc", "-c", command, "test", str(project / "completions/srm.bash"), *words],
                            env=env, cwd=root, capture_output=True, text=True, check=True)
    return result.stdout.splitlines()


assert "restore" in bash(["srm", "res"])
assert "status" in bash(["srm", "remote", "st"])
assert "--target" in bash(["srm", "remote", "restore", "--t"])
assert "two words.txt" in bash(["srm", "restore", "-t", "two"])
assert "two words.txt" in bash(["srm", "--", "two"])
assert "--json" in bash(["srm", "--config", str(root / "unused.json"), "list", "--j"])
assert "--interactive=always" in bash(["srm", "--rm", "--interactive=a"])
assert "30" in bash(["srm", "del", "-d", "3"])
assert "f_0123456789abcdef" in bash(["srm", "restore", "f_0"])
assert "f_0123456789abcdef" not in bash(["sudo", "srm", "restore", "f_0"])
assert "--dry-run" in bash(["srm", "clean", "--d"])
assert record.exists() and (root / "two words.txt").read_text() == "unchanged"
print("Bash: 11 completion/no-side-effect checks passed")

zsh_code = r'''
_describe() { local array=$2; print -l -- "${(@P)array}"; }
_files() { print FILE_CANDIDATES; }
compadd() { print -l -- "$@"; }
_test_srm() { source "$SRM_ZSH_TEST"; }
words=(srm ''); CURRENT=2; _test_srm
words=(srm remote ''); CURRENT=3; _test_srm
words=(srm restore -t ''); CURRENT=4; _test_srm
words=(srm restore f_0); CURRENT=3; _test_srm
'''
result = subprocess.run(["zsh", "-f", "-c", zsh_code], env={**env, "LC_ALL": "C.UTF-8", "SRM_LANG": "zh", "SRM_ZSH_TEST": str(project / "_srm")},
                        capture_output=True, text=True, check=True)
assert "restore:按 ID 恢复文件" in result.stdout
assert "sync:上传待处理队列" in result.stdout
assert "FILE_CANDIDATES" in result.stdout
assert "f_0123456789abcdef:/data/two words" in result.stdout, result.stdout + result.stderr
print("Zsh: command descriptions, remote descriptions, target paths and ID descriptions passed")
english = subprocess.run(["zsh", "-f", "-c", zsh_code], env={**env, "LC_ALL": "C", "SRM_LANG": "zh", "SRM_ZSH_TEST": str(project / "_srm")}, capture_output=True, text=True, check=True)
assert "restore:Restore by ID" in english.stdout
assert "restore:按 ID" not in english.stdout
assert "zh" in bash(["srm", "--lang", "z"])
assert "--json" in bash(["srm", "--lang", "zh", "list", "--j"])
print("Completion language selection and non-UTF-8 fallback passed")
cfg = root / "language.json"
cfg.write_text(json.dumps({"language": "zh"}))
cfg.chmod(0o600)
stored_env = {**env, "LC_ALL": "C.UTF-8", "SRM_CONFIG": str(cfg), "SRM_ZSH_TEST": str(project / "_srm")}
stored_env.pop("SRM_LANG", None)
stored = subprocess.run(["zsh", "-f", "-c", zsh_code], env=stored_env, capture_output=True, text=True, check=True)
assert "restore:按 ID" in stored.stdout
print("Zsh persisted configuration language passed")
print("Evidence:", root)

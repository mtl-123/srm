#!/usr/bin/env python3
"""Linux integration and differential GNU rm tests; only disposable fixtures are deleted."""
import hashlib
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

BINARY = Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else Path("target/debug/srm").resolve()
ROOT = Path(tempfile.mkdtemp(prefix="srm-v2-tests-", dir=Path.home()))


def fixture(p):
    p.mkdir()
    for name in ("a", "b", "c", "d", "-dash", "--", "list", "restore", "two words", "中文", "line\nbreak", "file..txt", os.fsdecode(b"bad-\xff")):
        (p / name).write_text("payload:" + repr(name))
    (p / "empty").mkdir()
    (p / "tree/nested").mkdir(parents=True)
    (p / "tree/top").write_text("top")
    (p / "tree/nested/leaf").write_text("leaf")
    (p / "link").symlink_to("a")
    (p / "dangling").symlink_to("missing")
    (p / "dirlink").symlink_to("tree")


def snapshot(p):
    result = []
    for path in sorted(p.rglob("*")):
        if path.is_symlink():
            value = ("link", os.readlink(path))
        elif path.is_dir():
            value = ("dir",)
        else:
            value = ("file", path.read_bytes(), path.stat().st_mode & 0o777)
        result.append((str(path.relative_to(p)), value))
    return result


class Integration(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="case-", dir=ROOT))
        self.data = self.root / "data"
        fixture(self.data)
        self.trash = self.root / "private-trash"
        self.env = {**os.environ, "SRM_HOME": str(self.trash), "LC_ALL": "C", "TZ": "UTC"}
        self.env.pop("POSIXLY_CORRECT", None)
        self.rm = self.root / "rm"
        self.rm.symlink_to(BINARY)

    def call(self, *args, input="", rm=False, check=True, env=None):
        p = subprocess.run([str(self.rm if rm else BINARY), *map(str, args)], cwd=self.data,
                           env=env or self.env, input=input, capture_output=True, text=True,
                           errors="backslashreplace", timeout=30)
        if check:
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        return p

    def records(self):
        return json.loads(self.call("list", "--json").stdout)

    def test_file_and_directory_roundtrip(self):
        before = snapshot(self.data)
        self.call("-r", "tree", "a", "link", "dangling", rm=True)
        rows = self.records()
        self.assertEqual(len(rows), 4)
        for r in rows:
            self.call("restore", r["id"])
        self.assertEqual(snapshot(self.data), before)

    def test_symlink_does_not_chmod_target(self):
        (self.data / "a").chmod(0o600)
        (self.data / "absolute-link").symlink_to(self.data / "a")
        self.call("absolute-link", rm=True)
        self.call("restore", self.records()[0]["id"])
        self.assertEqual((self.data / "a").stat().st_mode & 0o777, 0o600)

    def test_clean_files_directories_and_links(self):
        self.call("del", "--expire-days", "0", "a", "tree", "link", "dangling")
        self.call("clean", "--dry-run")
        self.assertEqual(len(self.records()), 4)
        self.call("clean")
        self.assertEqual(self.records(), [])
        self.assertEqual(list((self.trash / "files").iterdir()), [])
        self.assertTrue((self.data / "b").exists())

    def test_empty_cancel_and_confirm(self):
        self.call("a", rm=True)
        self.call("empty", input="n\n")
        self.assertEqual(len(self.records()), 1)
        self.call("empty", "-y")
        self.assertEqual(self.records(), [])

    def test_expiration_preserves_active(self):
        self.call("a", rm=True)
        self.call("del", "-d", "0", "b")
        self.call("clean")
        self.assertEqual(len(self.records()), 1)

    def test_force_restore_backs_up_collision(self):
        original = (self.data / "a").read_bytes()
        self.call("a", rm=True)
        item = self.records()[0]
        (self.data / "a").write_text("replacement")
        self.call("res", item["id"], input="n\n")
        self.assertEqual((self.data / "a").read_text(), "replacement")
        self.call("res", "-f", item["id"])
        self.assertEqual((self.data / "a").read_bytes(), original)
        backup = self.records()[0]
        self.assertEqual(Path(backup["trash_path"]).read_text(), "replacement")

    def test_non_utf8_restore(self):
        name = os.fsdecode(b"bad-\xff")
        content = (self.data / name).read_bytes()
        self.call(name, rm=True)
        self.call("res", self.records()[0]["id"])
        self.assertEqual((self.data / name).read_bytes(), content)

    def test_sparse_same_fs(self):
        p = self.data / "sparse"
        with p.open("wb") as f:
            f.truncate(1024**4)
        inode = p.stat().st_ino
        self.call("sparse", rm=True)
        r = self.records()[0]
        self.assertEqual(Path(r["trash_path"]).stat().st_ino, inode)
        self.call("res", r["id"])
        self.assertEqual(p.stat().st_size, 1024**4)
        self.assertEqual(p.stat().st_blocks, 0)

    def test_cross_filesystem_tree(self):
        cross = Path(tempfile.mkdtemp(prefix="srm-v2-", dir="/dev/shm"))
        self.addCleanup(shutil.rmtree, cross)
        tree = cross / "tree"
        (tree / "nested").mkdir(parents=True)
        p = tree / "nested/data"
        p.write_bytes(b"abc12345" * 2**21)
        p.chmod(0o600)
        os.utime(p, ns=(1000000000000000000, 1000000000000000000))
        os.link(p, tree / "hardlink")
        (tree / "relative").symlink_to("nested/data")
        (tree / "dangling").symlink_to("missing")
        before = snapshot(tree)
        self.call("-r", tree, rm=True)
        self.assertFalse(tree.exists())
        r = self.records()[0]
        backup = Path(r["trash_path"])
        self.assertEqual(snapshot(backup), before)
        self.assertEqual((backup / "hardlink").stat().st_ino, (backup / "nested/data").stat().st_ino)
        self.call("res", r["id"])
        self.assertEqual(snapshot(tree), before)
        self.assertEqual(p.stat().st_mtime_ns, 1000000000000000000)
        self.assertEqual((tree / "hardlink").stat().st_ino, p.stat().st_ino)

    def test_private_permissions_and_audit(self):
        self.call("a", rm=True)
        row = self.records()[0]
        for path in (self.trash, self.trash / "meta", self.trash / "files"):
            self.assertEqual(path.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.trash / "meta" / (row["id"] + ".json")).stat().st_mode & 0o777, 0o600)
        log = self.trash / "audit.jsonl"
        self.assertEqual(log.stat().st_mode & 0o777, 0o600)
        records = [json.loads(l) for l in log.read_text().splitlines()]
        self.assertEqual(records[-1]["action"], "delete_complete")
        self.assertEqual(records[-1]["original_path"], str(self.data / "a"))

    def test_concurrent_deletes(self):
        commands = []
        for i in range(12):
            p = self.data / f"parallel-{i}"
            p.write_text(str(i))
            commands.append(subprocess.Popen([str(self.rm), str(p)], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE))
        for p in commands:
            out, err = p.communicate(timeout=30)
            self.assertEqual(p.returncode, 0, out + err)
        rows = self.records()
        self.assertEqual(len(rows), 12)
        self.assertEqual(len({r["id"] for r in rows}), 12)
        self.call("doctor")

    def test_damaged_metadata_is_retained(self):
        self.call("a", rm=True)
        r = self.records()[0]
        meta = self.trash / "meta" / (r["id"] + ".json")
        meta.write_text("broken")
        self.assertNotEqual(self.call("list", check=False).returncode, 0)
        self.assertNotEqual(self.call("clean", "--all", check=False).returncode, 0)
        self.assertNotEqual(self.call("doctor", check=False).returncode, 0)
        self.assertTrue(meta.exists())
        self.assertTrue(Path(r["trash_path"]).exists())

    def test_trash_self_and_ancestor_protection(self):
        self.call("a", rm=True)
        for target in (self.trash, self.root, self.trash / "files"):
            self.assertNotEqual(self.call("-rf", target, rm=True, check=False).returncode, 0)
        self.assertEqual(len(self.records()), 1)

    def test_root_protection(self):
        self.assertNotEqual(self.call("-rf", "/", rm=True, check=False).returncode, 0)

    def test_help_has_no_storage_side_effect(self):
        self.call("--help", rm=True)
        self.assertFalse(self.trash.exists())

    def test_custom_restore_and_prefix_id(self):
        self.call("a", rm=True)
        r = self.records()[0]
        self.call("res", r["id"][:8], "-t", self.data / "new/parent/restored")
        self.assertTrue((self.data / "new/parent/restored").exists())

    def test_bad_multi_target_restore_is_not_destructive(self):
        self.call("a", "b", rm=True)
        ids = [r["id"] for r in self.records()]
        self.assertNotEqual(self.call("res", *ids, "-t", "new-name", check=False).returncode, 0)
        self.assertEqual(len(self.records()), 2)

    def test_metadata_write_failure_keeps_source(self):
        self.call("list")
        (self.trash / "meta").chmod(0o500)
        try:
            self.assertNotEqual(self.call("a", rm=True, check=False).returncode, 0)
            self.assertTrue((self.data / "a").exists())
        finally:
            (self.trash / "meta").chmod(0o700)

    def test_no_follow_trash_symlink(self):
        self.trash.symlink_to(self.data)
        self.assertNotEqual(self.call("a", rm=True, check=False).returncode, 0)
        self.assertTrue((self.data / "a").exists())

    def test_trailing_slash_symlink_is_intentionally_rejected(self):
        before = snapshot(self.data)
        self.assertNotEqual(self.call("-rf", "dirlink/", rm=True, check=False).returncode, 0)
        self.assertEqual(snapshot(self.data), before)

    def test_completed_rename_journal_recovery(self):
        self.call("a", rm=True)
        r = self.records()[0]
        path = self.trash / "meta" / (r["id"] + ".json")
        record = json.loads(path.read_text())
        record["state"] = "prepared"
        path.write_text(json.dumps(record))
        self.assertNotEqual(self.call("doctor", check=False).returncode, 0)
        self.call("recover", r["id"])
        self.call("res", r["id"])
        self.assertTrue((self.data / "a").exists())

    def test_partial_copy_is_never_marked_complete(self):
        self.call("a", rm=True)
        r = self.records()[0]
        path = self.trash / "meta" / (r["id"] + ".json")
        record = json.loads(path.read_text())
        record["state"] = "copying"
        path.write_text(json.dumps(record))
        self.assertNotEqual(self.call("recover", r["id"], check=False).returncode, 0)
        self.assertNotEqual(self.call("empty", "-y", check=False).returncode, 0)
        self.assertTrue(Path(r["trash_path"]).exists())

    def test_readonly_nested_directory_matches_gnu(self):
        native = self.root / "native"
        fixture(native)
        for p in (self.data, native):
            (p / "tree/nested").chmod(0o500)
        try:
            expected = subprocess.run(["/bin/rm", "-rf", "tree"], cwd=native, capture_output=True)
            got = self.call("-rf", "tree", rm=True, check=False)
            self.assertEqual(got.returncode, expected.returncode)
            self.assertEqual(snapshot(self.data), snapshot(native))
        finally:
            for p in (self.data, native):
                if (p / "tree/nested").exists():
                    (p / "tree/nested").chmod(0o700)

    def test_interrupt_while_waiting_for_lock(self):
        self.call("list")
        with (self.trash / "lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            process = subprocess.Popen([str(self.rm), "a"], cwd=self.data, env=self.env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            time.sleep(0.15)
            process.send_signal(signal.SIGINT)
            out, err = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 130, out + err)
        self.assertTrue((self.data / "a").exists())

    def test_interrupt_during_cross_copy_preserves_source(self):
        cross = Path(tempfile.mkdtemp(prefix="srm-v2-interrupt-", dir="/dev/shm"))
        self.addCleanup(shutil.rmtree, cross)
        source = cross / "large"
        with source.open("wb") as file:
            for _ in range(256):
                file.write(b"not-sparse-data!" * 65536)
        process = subprocess.Popen([str(self.rm), str(source)], env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 10
        signaled = False
        while process.poll() is None and time.monotonic() < deadline:
            for path in (self.trash / "meta").glob("*.json"):
                record = json.loads(path.read_text())
                if record["state"] == "copying":
                    process.send_signal(signal.SIGINT)
                    signaled = True
                    break
            if signaled:
                break
            time.sleep(0.001)
        out, err = process.communicate(timeout=30)
        self.assertTrue(signaled, "copy completed before the interrupt could be injected")
        self.assertEqual(process.returncode, 130, out + err)
        self.assertTrue(source.exists())
        self.assertEqual(source.stat().st_size, 256 * 1024 * 1024)
        self.assertNotEqual(self.call("doctor", check=False).returncode, 0)

    def test_xattr_and_mode_cross_file(self):
        cross = Path(tempfile.mkdtemp(prefix="srm-v2-xattr-", dir="/dev/shm"))
        self.addCleanup(shutil.rmtree, cross)
        source = cross / "file"
        source.write_bytes(b"extended metadata")
        source.chmod(0o640)
        os.setxattr(source, "user.srm-test", b"keep-me")
        self.call(source, rm=True)
        r = self.records()[0]
        self.assertEqual(os.getxattr(r["trash_path"], "user.srm-test"), b"keep-me")
        self.call("res", r["id"])
        self.assertEqual(os.getxattr(source, "user.srm-test"), b"keep-me")
        self.assertEqual(source.stat().st_mode & 0o777, 0o640)

    def test_install_activate_and_uninstall(self):
        home = self.root / "fake-home"
        home.mkdir()
        (home / ".bashrc").write_text("# existing config\n")
        env = {**self.env, "HOME": str(home)}
        scripts = Path(__file__).resolve().parents[1] / "scripts"
        args = ["bash", str(scripts / "install.sh"), "--binary", str(BINARY), "--replace-rm", "--activate-bash"]
        for _ in range(2):
            installed = subprocess.run(args, env=env, capture_output=True, text=True)
            self.assertEqual(installed.returncode, 0, installed.stderr)
        self.assertEqual((home / ".bashrc").read_text().count("# safe-srm managed activation"), 1)
        command = 'source "$HOME/.config/safe-srm/env.sh"; [[ $(type -t rm) = function ]] || exit 1; rm -- a'
        activated = subprocess.run(["bash", "--noprofile", "--norc", "-c", command], cwd=self.data, env=env,
                                   capture_output=True, text=True)
        self.assertEqual(activated.returncode, 0, activated.stderr)
        self.assertFalse((self.data / "a").exists())
        self.assertEqual(len(self.records()), 1)
        subprocess.run(["bash", str(scripts / "uninstall.sh")], env=env, check=True, capture_output=True)
        self.assertFalse((home / ".local/bin/rm").is_symlink())
        self.assertEqual(len(self.records()), 1)

    def test_installer_refuses_foreign_rm(self):
        home = self.root / "fake-home"
        (home / ".local/bin").mkdir(parents=True)
        other = home / ".local/bin/rm"
        other.write_text("do not overwrite")
        scripts = Path(__file__).resolve().parents[1] / "scripts"
        installed = subprocess.run(["bash", str(scripts / "install.sh"), "--binary", str(BINARY), "--replace-rm"],
                                   env={**self.env, "HOME": str(home)}, capture_output=True, text=True)
        self.assertNotEqual(installed.returncode, 0)
        self.assertEqual(other.read_text(), "do not overwrite")

    def test_batch_group_roundtrip_and_cleanup(self):
        paths = []
        for i in range(600):
            p = self.data / f"batch-{i}"
            p.write_text(str(i))
            paths.append(p)
        self.call(*paths, rm=True)
        rows = self.records()
        self.assertEqual(len(rows), 600)
        self.assertEqual(len(list((self.trash / "meta").glob("*.json"))), 3)
        for row in (rows[0], rows[255], rows[-1]):
            self.call("restore", row["id"])
            p = Path(row["original_path"])
            self.assertEqual(p.read_text(), p.name.split("-")[-1])
        self.call("empty", "-y")
        self.assertEqual(self.records(), [])

    def test_configurable_fast_directory(self):
        config = self.root / "config.json"
        config.write_text(json.dumps({"local_store": str(self.trash), "directory_strategy": "rename"}))
        config.chmod(0o600)
        before = snapshot(self.data / "tree")
        self.call("--config", config, "--rm", "-rf", "tree")
        rows = self.records()
        self.assertEqual(len(rows), 1)
        self.call("restore", rows[0]["id"])
        self.assertEqual(snapshot(self.data / "tree"), before)

    def test_invalid_configuration_fails_before_deletion(self):
        config = self.root / "config.json"
        config.write_text('{"backend":"remote_async"}')
        config.chmod(0o600)
        self.assertNotEqual(self.call("--config", config, "--rm", "a", check=False).returncode, 0)
        self.assertTrue((self.data / "a").exists())

    def test_volume_local_store_avoids_cross_device_copy(self):
        cross = Path(tempfile.mkdtemp(prefix="srm-volume-", dir="/dev/shm"))
        self.addCleanup(shutil.rmtree, cross)
        source = cross / "large"
        source.write_bytes(b"volume payload")
        inode = source.stat().st_ino
        config = self.root / "config.json"
        config.write_text(json.dumps({"local_store": str(self.trash), "volume_stores": [str(cross / "volume-trash")]}))
        config.chmod(0o600)
        self.call("--config", config, "--rm", source)
        rows = self.records()
        backup = Path(rows[0]["trash_path"])
        self.assertEqual(backup.stat().st_ino, inode)
        self.assertTrue(backup.is_relative_to(cross))
        self.call("restore", rows[0]["id"])
        self.assertEqual(source.stat().st_ino, inode)
        self.assertEqual(source.read_bytes(), b"volume payload")


CASES = [
    ([], ""), (["-f"], ""), (["missing"], ""), (["-f", "missing"], ""),
    (["-f", "missing/child"], ""), (["-f", "a/child"], ""),
    ([""], ""), (["-f", ""], ""), (["a"], ""), (["a", "missing", "b"], ""),
    (["-f", "a", "a"], ""), (["a", "a"], ""),
    (["a/"], ""), (["-rf", "a/"], ""), (["a/.", "b"], ""),
    (["empty"], ""), (["-d", "empty"], ""), (["-d", "tree"], ""),
    (["-r", "tree"], ""), (["-rf", "tree"], ""), (["-R", "tree"], ""),
    (["--recursive", "tree"], ""), (["--rec", "tree"], ""),
    (["tree", "-r"], ""), (["--", "-dash"], ""), (["--", "--"], ""),
    (["-dash"], ""), (["two words", "中文", "line\nbreak", "file..txt"], ""),
    ([os.fsdecode(b"bad-\xff")], ""), (["list", "restore"], ""),
    (["link", "dangling", "dirlink"], ""),
    (["-rf", "."], ""), (["-rf", ".."], ""), (["-rf", "tree/."], ""),
    (["-rf", "tree/.."], ""), (["-r", "tree/"], ""),
    (["-i", "a"], "n\n"), (["-i", "a"], "y\n"),
    (["-fi", "a"], "n\n"), (["-if", "a"], "n\n"),
    (["-I", "a", "b", "c", "d"], "n\n"), (["-I", "a", "b", "c", "d"], "y\n"),
    (["-I", "a"], "n\n"), (["-rI", "tree"], "n\n"),
    (["-ri", "tree"], "y\n" * 20), (["-ri", "tree"], "n\n" * 20),
    (["-ri", "tree"], "y\nn\ny\nn\ny\nn\n"),
    (["--interactive", "a"], "n\n"), (["--interactive=never", "a"], "n\n"),
    (["--interactive=once", "-r", "tree"], "y\n"),
    (["-r", "--one-file-system", "tree"], ""),
    (["--preserve-root=all", "a"], ""), (["--no-preserve-root", "a"], ""),
    (["--force=yes", "a"], ""), (["--interactive=bogus", "a"], ""),
    (["--preserve-root=bad", "a"], ""), (["--unknown", "a"], ""),
    (["--v", "a"], ""), (["-h"], ""), (["-V"], ""),
    (["-fI", "missing"], ""), (["-f", "--interactive=never", "missing"], ""),
]


def differential(args, answers):
    def test(self):
        native = self.root / "native"
        fixture(native)
        before = snapshot(self.data)
        expected = subprocess.run(["/bin/rm", *args], cwd=native, env=self.env, input=answers,
                                  capture_output=True, text=True, errors="backslashreplace", timeout=30)
        got = self.call(*args, input=answers, rm=True, check=False)
        self.assertEqual(got.returncode, expected.returncode,
                         f"{args!r}\nGNU: {expected.stderr}\nsrm: {got.stderr}")
        self.assertEqual(snapshot(self.data), snapshot(native), f"{args!r}\n{got.stderr}")
        if got.returncode == 0 and before != snapshot(self.data):
            self.assertGreater(len(self.records()), 0)
    return test


for index, (args, answers) in enumerate(CASES):
    setattr(Integration, f"test_gnu_{index:02d}", differential(args, answers))

if __name__ == "__main__":
    print("Disposable test root:", ROOT, flush=True)
    unittest.main(verbosity=2)

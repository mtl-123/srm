# x86_64 Linux portable release

This release targets 64-bit x86 Linux (amd64/x86_64), using the baseline
x86-64 CPU target and statically linked musl. It does not require a target
glibc version or AVX/AVX2 instructions. It is not an i386/32-bit or ARM build.

The ELF has no interpreter (PT_INTERP) and no NEEDED shared libraries.
This does not remove kernel, filesystem, or external utility requirements.

## Standalone installation: keep system rm unchanged

After extracting the archive:

```bash
bash scripts/install.sh --binary ./srm
# Or install a root-owned srm command for all users:
sudo bash scripts/install-system.sh --binary ./srm
```

Do not pass `--replace-rm`. The binary is called `srm`; system `rm` is retained.
For an existing shell, add ~/.local/bin to PATH or invoke the installed path.

```bash
srm file.txt
srm -rf directory/
sudo /usr/local/bin/srm /root/example.txt
srm list
srm restore ID
```

## Requirements by feature

| Feature | Requirements |
|---|---|
| Local same-filesystem recovery | Linux /proc, renameat2 with RENAME_NOREPLACE, supported filesystem, working file/directory fsync and advisory locking |
| Cross-filesystem copying | GNU coreutils `/bin/cp` supporting archive, reflink and sparse options |
| Remote SSH archives | `/usr/bin/ssh`, GNU `/bin/tar`, local `/usr/bin/python3`; SSH server and Python 3 on the remote host |
| Installation scripts | Bash and standard GNU installation utilities |
| Optional timers | systemd; the binary and manual management commands do not require systemd |

Use a current supported Linux distribution and kernel. Linux 3.15 introduced
renameat2, but this is not a claim that every 3.15-era kernel/filesystem is
supported: flag support, syscall filtering and filesystem semantics also matter.
An unsupported operation must fail rather than silently fall back to an
overwriting rename. Legacy CentOS 7 kernels are not a guaranteed target.

On Alpine, basic same-filesystem commands work without glibc; install Bash,
coreutils, GNU tar, openssh-client and python3 for the respective optional
features. BusyBox cp/tar are not substitutes for the GNU options used here.

Typical dependency package names:

```bash
# Debian / Ubuntu
sudo apt-get install bash coreutils tar openssh-client python3
# Rocky / Alma / RHEL / Fedora
sudo dnf install bash coreutils tar openssh-clients python3
# Alpine
sudo apk add bash coreutils tar openssh-client python3
# openSUSE
sudo zypper install bash coreutils tar openssh python3
```

## Verification limits

The portable binary passes the local integration suite on WSL Debian 13.
Distribution container smoke tests cover startup, same-filesystem directory
deletion, a dangling symlink, restoration, expiration cleanup and doctor.
Containers share the host Linux kernel, so these are userspace compatibility
tests, not independent distribution-kernel or filesystem certification.

NFS, SMB, FUSE, unusual mount configurations, old kernels, and all possible
security policy combinations have not been certified. Remote end-to-end tests
were performed on a temporary loopback SSH server, not every distribution.

Check the accompanying compatibility-results.json for exact tested images and
outcomes. Do not interpret a static build as a guarantee of universal safety.

The existing safety limitations documented in README.md remain applicable:
this release build does not implement the additional concurrency and crash
hardening proposals discussed after v2.1.0 was developed.

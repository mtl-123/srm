#!/usr/bin/env bash
set -euo pipefail
[[ $EUID = 0 ]] || { echo 'Run this system installer with sudo.' >&2; exit 1; }
project="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$project/target/release/srm"
[[ ! -x "$project/srm" ]] || binary="$project/srm"
replace_rm=false
enable_timer=false
while (($#)); do
    case "$1" in
        --binary) binary="${2:?missing binary}"; shift 2 ;;
        --replace-rm) replace_rm=true; shift ;;
        --enable-upload-timer) enable_timer=true; shift ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; exit 1 ;;
    esac
done
[[ -x "$binary" ]] || { echo 'Build the release executable first.' >&2; exit 1; }
if ! "$binary" --version; then
    echo "Cannot execute binary; use the x86_64-unknown-linux-musl static package." >&2
    exit 1
fi
destination=/usr/local/lib/safe-srm
names=(srm)
$replace_rm && names+=(rm)
for name in "${names[@]}"; do
    link="/usr/local/bin/$name"
    if [[ -e "$link" || -L "$link" ]]; then
        [[ -L "$link" && "$(readlink -- "$link")" = "$destination/srm" ]] || {
            printf 'Refusing to overwrite unrelated system command: %s\n' "$link" >&2; exit 1;
        }
    fi
done
install -d -o root -g root -m 0755 "$destination" /usr/local/bin /etc/safe-srm
install -d -o root -g root -m 0755 /usr/share/bash-completion/completions /usr/local/share/zsh/site-functions /usr/local/share/safe-srm
install -o root -g root -m 0644 "$project/completions/srm.bash" /usr/share/bash-completion/completions/srm
zsh_source="$project/_srm"
[[ -f "$zsh_source" ]] || zsh_source="$project/completions/_srm"
install -o root -g root -m 0644 "$zsh_source" /usr/local/share/zsh/site-functions/_srm
install -o root -g root -m 0644 "$project/completions/completion_ids.py" /usr/local/share/safe-srm/completion_ids.py
install -o root -g root -m 0755 "$binary" "$destination/srm.new"
mv -f -- "$destination/srm.new" "$destination/srm"
for name in "${names[@]}"; do ln -sfn -- "$destination/srm" "/usr/local/bin/$name"; done
if [[ ! -e /etc/safe-srm/config.json ]]; then "$destination/srm" config --init; fi
if $enable_timer; then
    install -m 0644 "$project/systemd/safe-srm-upload-system.service" /etc/systemd/system/safe-srm-upload.service
    install -m 0644 "$project/systemd/safe-srm-upload.timer" /etc/systemd/system/safe-srm-upload.timer
    systemctl daemon-reload
    systemctl enable --now safe-srm-upload.timer
fi
printf 'Installed root-owned executable: %s/srm\n' "$destination"
printf 'Root config: /etc/safe-srm/config.json\nRoot store: /var/lib/safe-srm/root (unless configured otherwise)\n'
printf 'Use: sudo /usr/local/bin/srm --rm FILE\n'
printf 'sudo rm uses this implementation only when sudo secure_path includes /usr/local/bin before /usr/bin.\n'
"$destination/srm" doctor >/dev/null

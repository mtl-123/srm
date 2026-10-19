#!/usr/bin/env bash
set -euo pipefail
destination="$HOME/.local/lib/safe-srm/srm"
if command -v systemctl >/dev/null && [[ -f "$HOME/.config/systemd/user/safe-srm-upload.timer" ]]; then
    systemctl --user disable --now safe-srm-upload.timer || true
    systemctl --user stop safe-srm-upload.service || true
fi
for name in srm rm; do
    link="$HOME/.local/bin/$name"
    if [[ -L "$link" && "$(readlink -- "$link")" = "$destination" ]]; then
        /bin/rm -- "$link"
    fi
done
# Leave the guarded .bashrc activation harmless; preserve backups and all trash.
if [[ -f "$HOME/.config/safe-srm/env.sh" ]]; then
    /bin/rm -- "$HOME/.config/safe-srm/env.sh"
fi
printf 'User command links removed; binary and trash retained for recovery.\n'
printf 'In the current shell run: unset -f rm; unalias rm 2>/dev/null; hash -r\n'

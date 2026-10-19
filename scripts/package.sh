#!/usr/bin/env bash
set -euo pipefail
project="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${1:-$project/target/release/srm}"
label="${2:-linux-x86_64-gnu}"
[[ "$label" =~ ^[a-zA-Z0-9_-]+$ ]] || { echo 'Invalid package label' >&2; exit 1; }
version="$("$binary" --version)"
version="${version##* }"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'Invalid executable version' >&2; exit 1; }
destination="$project/dist/safe-srm-$version-$label"
[[ ! -e "$destination" ]] || { echo 'Package directory already exists' >&2; exit 1; }
mkdir -p -- "$destination/scripts" "$destination/completions" "$destination/systemd"
mkdir -p -- "$destination/docs" "$destination/config/examples"
install -m 0755 -- "$binary" "$destination/srm"
if [[ "${3:-}" = --upx ]]; then
    upx --best --lzma "$destination/srm"
    upx -t "$destination/srm"
fi
cp -- "$project/README.md" "$project/LICENSE" "$destination/"
cp -- "$project/INSTALL.txt" "$destination/"
cp -- "$project/docs/"*.md "$destination/docs/"
cp -- "$project/config/examples/"*.json "$destination/config/examples/"
cp -- "$project/scripts/install.sh" "$project/scripts/install-system.sh" "$project/scripts/uninstall.sh" "$destination/scripts/"
cp -- "$project/completions/srm.bash" "$destination/completions/"
cp -- "$project/completions/completion_ids.py" "$destination/completions/"
cp -- "$project/_srm" "$destination/completions/"
cp -- "$project/systemd/"* "$destination/systemd/"
"$destination/srm" --version
tar -C "$project/dist" -czf "$destination.tar.gz" "$(basename -- "$destination")"
(cd -- "$project/dist" && sha256sum "$(basename -- "$destination").tar.gz" > "$(basename -- "$destination").tar.gz.sha256")
standalone="srm-$version-$label"
install -m 0755 -- "$destination/srm" "$project/dist/$standalone"
(cd -- "$project/dist" && sha256sum "$standalone" > "$standalone.sha256")
printf '%s\n' "$destination.tar.gz"

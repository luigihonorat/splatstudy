#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The SplatStudy contributors
# SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
# PostToolUse hook for Write: warns Claude when a Rust or WGSL file it just
# wrote has no SPDX licence header (REUSE needs one in every source file).
# Exit code 2 sends the message back to Claude; the file is already written.

input=$(cat)

# The file_path field of the tool input, with JSON's escaped backslashes
# turned into slashes (Windows paths).
path=$(printf '%s' "$input" \
  | grep -oE '"file_path"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"' \
  | head -n 1 \
  | sed -E 's/^"file_path"[[:space:]]*:[[:space:]]*"//; s/"$//; s#\\\\#/#g')

case "$path" in
  *.rs | *.wgsl) ;;
  *) exit 0 ;;
esac
[ -f "$path" ] || exit 0

if ! head -n 5 "$path" | grep -q 'SPDX-License-Identifier:'; then
  cat >&2 <<EOF
$path has no SPDX header. Start it with:
// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
EOF
  exit 2
fi
exit 0

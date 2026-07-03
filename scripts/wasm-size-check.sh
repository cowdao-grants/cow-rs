#!/usr/bin/env bash
# Assert the post-wasm-opt cow_sdk_wasm_bg.wasm fits under a byte
# ceiling. Used by `just wasm-size-check` and the wasm-size CI job.
#
# Usage: wasm-size-check.sh <ceiling-bytes>
#
# If the wasm exceeds the ceiling, the script prints a GitHub Actions
# error annotation and exits non-zero so the CI job fails. If we ever
# legitimately need more headroom (new API surface, dep bump), raise
# the ceiling in `justfile` and `.github/workflows/ci.yml` together
# with a commit message explaining the cause.

set -euo pipefail

ceiling="${1:?missing ceiling argument}"
wasm="crates/cow-sdk-wasm/pkg-web/cow_sdk_wasm_bg.wasm"

if [ ! -f "$wasm" ]; then
    echo "::error::$wasm not found; build it first with wasm-build-web"
    exit 1
fi

size=$(wc -c < "$wasm")
human=$(du -h "$wasm" | cut -f1)
ceiling_kb=$(( ceiling / 1024 ))

echo "$wasm: $human ($size bytes), ceiling ${ceiling_kb} KB ($ceiling bytes)"

if [ "$size" -gt "$ceiling" ]; then
    echo "::error::wasm size $size bytes exceeds ceiling $ceiling bytes ($human)"
    echo "If intentional, raise the ceiling in justfile and .github/workflows/ci.yml"
    echo "and note why in the commit message."
    exit 1
fi

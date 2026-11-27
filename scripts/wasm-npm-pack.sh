#!/usr/bin/env bash
# Combine the three wasm-pack outputs (web / bundler / nodejs) into a
# single publishable npm package directory with a deduplicated `.wasm`
# binary and a unified `package.json` whose `exports` map routes the
# right JS glue per consumer environment.
#
# Layout produced under `crates/cow-sdk-wasm/pkg-npm/`:
#
#   cow_sdk_wasm_bg.wasm        -- one copy, shared by all three glues
#   cow_sdk_wasm.d.ts           -- shared types
#   cow_sdk_wasm_bg.wasm.d.ts   -- shared wasm-side types
#   web/cow_sdk_wasm.js         -- ES module for browsers
#   bundler/cow_sdk_wasm.js     -- webpack / Vite / Rollup entry
#   bundler/cow_sdk_wasm_bg.js  -- inner glue the bundler entry imports
#   nodejs/cow_sdk_wasm.js      -- CommonJS for Node 18+
#   package.json                -- unified, with `exports` conditional map
#   README.md / LICENSE
#
# Each per-target JS glue is patched to load the shared `.wasm` from
# one directory up (`../cow_sdk_wasm_bg.wasm` / `${__dirname}/../...`)
# instead of next to itself.
#
# Run after `just wasm-build-all`. Wired via `just npm-pack`.

set -euo pipefail

ROOT="crates/cow-sdk-wasm"
SRC_WEB="$ROOT/pkg-web"
SRC_BUNDLER="$ROOT/pkg-bundler"
SRC_NODEJS="$ROOT/pkg-nodejs"
OUT="$ROOT/pkg-npm"

for dir in "$SRC_WEB" "$SRC_BUNDLER" "$SRC_NODEJS"; do
    if [ ! -d "$dir" ]; then
        echo "::error::missing $dir; run 'just wasm-build-all' first"
        exit 1
    fi
done

# Refuse to dedupe if the per-target .wasm binaries diverge. They should
# be byte-identical -- wasm-pack only varies the JS glue across targets
# -- but check defensively so we never ship a binary that doesn't match
# what one of the glues was generated against.
sha_web=$(shasum -a 256 "$SRC_WEB/cow_sdk_wasm_bg.wasm" | cut -d' ' -f1)
sha_bundler=$(shasum -a 256 "$SRC_BUNDLER/cow_sdk_wasm_bg.wasm" | cut -d' ' -f1)
sha_nodejs=$(shasum -a 256 "$SRC_NODEJS/cow_sdk_wasm_bg.wasm" | cut -d' ' -f1)
if [ "$sha_web" != "$sha_bundler" ] || [ "$sha_web" != "$sha_nodejs" ]; then
    echo "::error::wasm bytes differ across targets, dedupe would corrupt one of them"
    echo "  web     $sha_web"
    echo "  bundler $sha_bundler"
    echo "  nodejs  $sha_nodejs"
    exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/web" "$OUT/bundler" "$OUT/nodejs"

# Shared artefacts (one copy each, served via the unified `exports`).
cp "$SRC_WEB/cow_sdk_wasm_bg.wasm" "$OUT/"
cp "$SRC_WEB/cow_sdk_wasm.d.ts" "$OUT/"
cp "$SRC_WEB/cow_sdk_wasm_bg.wasm.d.ts" "$OUT/"

# web target: `new URL('cow_sdk_wasm_bg.wasm', import.meta.url)` -> one
# level up.
cp "$SRC_WEB/cow_sdk_wasm.js" "$OUT/web/cow_sdk_wasm.js"
perl -pi -e "s{'cow_sdk_wasm_bg\.wasm'}{'../cow_sdk_wasm_bg.wasm'}g" \
    "$OUT/web/cow_sdk_wasm.js"

# bundler target: `import * as wasm from "./cow_sdk_wasm_bg.wasm"` ->
# one level up. The sibling `./cow_sdk_wasm_bg.js` stays in the same
# subdir so its relative import keeps working.
cp "$SRC_BUNDLER/cow_sdk_wasm.js" "$OUT/bundler/cow_sdk_wasm.js"
cp "$SRC_BUNDLER/cow_sdk_wasm_bg.js" "$OUT/bundler/cow_sdk_wasm_bg.js"
perl -pi -e 's{"\./cow_sdk_wasm_bg\.wasm"}{"../cow_sdk_wasm_bg.wasm"}g' \
    "$OUT/bundler/cow_sdk_wasm.js"

# nodejs target: wasm-pack emits CommonJS here (`require`, `module.exports`,
# `__dirname`). The unified package is `"type": "module"`, so a `.js`
# extension would make Node parse this glue as ESM and throw "exports is not
# defined in ES module scope". Emit it as `.cjs` so Node always treats it as
# CommonJS regardless of the package `type`. The wasm path is also rewritten
# one level up to the shared binary.
cp "$SRC_NODEJS/cow_sdk_wasm.js" "$OUT/nodejs/cow_sdk_wasm.cjs"
perl -pi -e 's{\$\{__dirname\}/cow_sdk_wasm_bg\.wasm}{\${__dirname}/../cow_sdk_wasm_bg.wasm}g' \
    "$OUT/nodejs/cow_sdk_wasm.cjs"

# Inherit the version + description + repository fields from the
# per-target package.json wasm-pack already wrote (cargo metadata is
# the source of truth) so we don't drift.
version=$(grep '"version"' "$SRC_WEB/package.json" | head -1 | sed -E 's/.*"version": *"([^"]+)".*/\1/')
description=$(grep '"description"' "$SRC_WEB/package.json" | head -1 | sed -E 's/.*"description": *"([^"]+)".*/\1/')

cat > "$OUT/package.json" <<EOF
{
  "name": "@cowdao-grants/cow-sdk-wasm",
  "version": "$version",
  "description": "$description",
  "license": "GPL-3.0-or-later",
  "repository": {
    "type": "git",
    "url": "https://github.com/cowdao-grants/cow-rs"
  },
  "type": "module",
  "main": "./nodejs/cow_sdk_wasm.cjs",
  "module": "./web/cow_sdk_wasm.js",
  "browser": "./web/cow_sdk_wasm.js",
  "types": "./cow_sdk_wasm.d.ts",
  "exports": {
    ".": {
      "types": "./cow_sdk_wasm.d.ts",
      "node": "./nodejs/cow_sdk_wasm.cjs",
      "browser": "./web/cow_sdk_wasm.js",
      "default": "./bundler/cow_sdk_wasm.js"
    },
    "./cow_sdk_wasm_bg.wasm": "./cow_sdk_wasm_bg.wasm"
  },
  "files": [
    "cow_sdk_wasm_bg.wasm",
    "cow_sdk_wasm.d.ts",
    "cow_sdk_wasm_bg.wasm.d.ts",
    "web/",
    "bundler/",
    "nodejs/",
    "README.md",
    "LICENSE"
  ],
  "sideEffects": false
}
EOF

cp "$ROOT/README.md" "$OUT/README.md"
cp LICENSE "$OUT/LICENSE"

# Quick summary so the developer can eyeball what got built.
echo ""
echo "wasm-npm-pack: assembled $OUT"
echo "  $(du -h "$OUT/cow_sdk_wasm_bg.wasm" | cut -f1)  cow_sdk_wasm_bg.wasm (shared)"
for tgt in web bundler nodejs; do
    glue="cow_sdk_wasm.js"
    [ "$tgt" = nodejs ] && glue="cow_sdk_wasm.cjs"
    js_size=$(du -h "$OUT/$tgt/$glue" | cut -f1)
    echo "  ${js_size}  $tgt/$glue"
done
total=$(du -sh "$OUT" | cut -f1)
echo "  ${total}  total"

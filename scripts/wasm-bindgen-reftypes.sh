#!/bin/sh
# worker-build 调用 wasm-bindgen 时的包装器：补上 --reference-types。
#
# 背景：worker-build 0.8.7 会带 --force-enable-abort-handler 生成 catch wrapper，
# 这要求 wasm 里有 externref 表；而它自己不会传 --reference-types，于是报
#   error: failed to generate catch wrappers
#   caused by: externref table required for catch wrappers
# （配合 .cargo/config.toml 里的 target-feature=+reference-types）。
#
# 用法（wrangler.toml 的 build.command 已内置）：
#   WASM_BINDGEN_BIN=scripts/wasm-bindgen-reftypes.sh worker-build --release
set -e

# 优先用 PATH 里的全局 wasm-bindgen（版本需与 Cargo.lock 中 wasm-bindgen 一致），
# 没有就退回 worker-build 自己下载的那份。
BIN="$(command -v wasm-bindgen || true)"
if [ -z "$BIN" ]; then
  BIN="$(ls "$HOME"/Library/Caches/worker-build/wasm-bindgen-*/wasm-bindgen 2>/dev/null | head -1)"
fi
if [ -z "$BIN" ]; then
  echo "wasm-bindgen not found (cargo install wasm-bindgen-cli --version <lock version> --locked)" >&2
  exit 1
fi

exec "$BIN" "$@" --reference-types

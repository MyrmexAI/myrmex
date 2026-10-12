#!/usr/bin/env bash
# Собрать программы в .so и прогнать их тесты: scripts/program-test.sh <prog> [<prog>...]
# Ключи программ из keys/ кладутся в target/deploy, чтобы адреса совпадали с declare_id!.
set -euo pipefail
export CARGO_HOME=/d/dev/.cargo RUSTUP_HOME=/d/dev/.rustup RUST_LOG=error
export PATH="/d/dev/.cargo/bin:/d/dev/solana/active_release/bin:$PATH"
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/target/deploy"
for p in "$@"; do
  cp "$root/keys/$p-keypair.json" "$root/target/deploy/"
  (cd "$root/programs/$p" && cargo build-sbf 2>&1 | grep -E "^error|warning: unused|Finished" || true)
  test -f "$root/target/deploy/$p.so" || { echo "нет $p.so"; exit 1; }
  for i in 1 2 3 4; do
    if cargo test -p "$p" > "$root/target/test-$p.log" 2>&1; then break; fi
    grep -q "os error 32" "$root/target/test-$p.log" || break
    echo "повтор (файл занят антивирусом)"
  done
  grep -E "^test |test result|panicked|^error" "$root/target/test-$p.log" | grep -v "0 passed; 0 failed" || true
done

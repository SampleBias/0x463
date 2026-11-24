#!/usr/bin/env bash
set -euo pipefail

work=$(mktemp -d "${RUNNER_TEMP:?}/sqlite-build.XXXXXXXX")
prefix="$work/install"
cd "$work"
curl --fail --silent --show-error --location --retry 2 \
  https://www.sqlite.org/2026/sqlite-autoconf-3540100.tar.gz -o sqlite.tar.gz
python3 - <<'PY'
from hashlib import sha3_256
from pathlib import Path

expected = "b6ced585c750916b1052667239a12c25a7ff34e7f06e1e6e01ecb0427657d9c4"
if sha3_256(Path("sqlite.tar.gz").read_bytes()).hexdigest() != expected:
    raise SystemExit("SQLite 3.54.1 source checksum mismatch")
PY
tar -xzf sqlite.tar.gz
cd sqlite-autoconf-3540100
CFLAGS="-O2 -DSQLITE_ENABLE_COLUMN_METADATA -DSQLITE_ENABLE_UNLOCK_NOTIFY" ./configure --prefix="$prefix"
make -j2
make install
echo "PKG_CONFIG_PATH=$prefix/lib/pkgconfig" >> "$GITHUB_ENV"
echo "LD_LIBRARY_PATH=$prefix/lib" >> "$GITHUB_ENV"
LD_LIBRARY_PATH="$prefix/lib" "$prefix/bin/sqlite3" -version
features=$(LD_LIBRARY_PATH="$prefix/lib" "$prefix/bin/sqlite3" :memory: \
  "SELECT sqlite_compileoption_used('ENABLE_COLUMN_METADATA'), sqlite_compileoption_used('ENABLE_UNLOCK_NOTIFY')")
test "$features" = '1|1'

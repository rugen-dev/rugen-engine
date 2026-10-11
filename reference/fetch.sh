#!/usr/bin/env bash
# Fetch reference material into reference/. Everything here is gitignored.
# Re-run safely; skips what exists.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p dl

# 1. Official MUGEN 1.1b1 (closed-source freeware, Elecbyte, 27 Jul 2013)
if [ ! -d mugen-1.1b1 ]; then
  curl -sL -o dl/mugen-1.1b1.zip https://archive.org/download/mugen-1.1b1/mugen-1.1b1.zip
  echo "8ab8e69877aea6f0cad1b9fa2970ba509638745f  dl/mugen-1.1b1.zip" | sha1sum -c -
  mkdir -p mugen-1.1b1 && unzip -qo dl/mugen-1.1b1.zip -d mugen-1.1b1
  mv mugen-1.1b1/mugen-1.1b1/* mugen-1.1b1/ && rmdir mugen-1.1b1/mugen-1.1b1
fi

# 2. Official 1.1b1 HTML docs
if [ ! -d mugen-docs ]; then
  mkdir -p mugen-docs && (cd mugen-docs && wget -q -m -np -nH --cut-dirs=1 -e robots=off -w 0.3 \
    https://www.elecbyte.com/mugendocs-11b1/mugen.html)
fi

# 3. Ikemen GO source (semantic spec)
[ -d Ikemen-GO ] || git clone --depth 1 https://github.com/ikemen-engine/Ikemen-GO.git

# 4. Ikemen GO v1.0.0 runtime + content pack (KFM, stages, screenpack)
if [ ! -d ikemen-go-v1.0.0 ]; then
  B=https://github.com/ikemen-engine/Ikemen-GO/releases/download/v1.0.0
  (cd dl && curl -sLO $B/SHA256SUMS.txt && curl -sLO $B/work.zip && curl -sLO $B/Ikemen_GO-v1.0.0-linux.zip \
    && sha256sum -c --ignore-missing SHA256SUMS.txt)
  mkdir -p ikemen-go-v1.0.0 && (cd ikemen-go-v1.0.0 && unzip -qo ../dl/Ikemen_GO-v1.0.0-linux.zip && unzip -qo ../dl/work.zip)
fi
echo "reference ready"

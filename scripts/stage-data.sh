#!/usr/bin/env bash
# Stage the MUGEN base data the packages ship into dist/pkgdata/.
#
#   scripts/stage-data.sh
#
# Characters are built on MUGEN's common states (data/common1.cns) and the
# lifebars use its hit sparks and sounds (fightfx.*, common.snd, fight.snd).
# Without them almost no fighter loads, so every package ships them in
# data/ (portable folder) or /usr/share/rugen/data (rpm / deb). They are
# Elecbyte's M.U.G.E.N 1.1 beta 1 base motif files, kept out of git like the
# rest of reference/ (run reference/fetch.sh first) and copied unmodified
# with Elecbyte's readme (license) and a notice. common1.cns must be exactly
# the 1.1b1 file: it is part of every fighter's online fingerprint.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
src=reference/mugen-1.1b1
out=dist/pkgdata
rm -rf "$out"; mkdir -p "$out"
(cd "$src" && sha256sum -c --quiet - <<'SUMS'
1b3fa7977d31884d3c7c3475a7238cdc6d463ac61f60a55daedaec757aff54d3  data/common1.cns
b49c8b62fb07a1212ce3e3d2c98bf5f59446ad68451b6a70a4740110f75d5926  data/common.snd
d9f6bc1d98682815f69a793f6a709163ad5beed971786eea8db2483221124bc7  data/fightfx.air
e65fcbb0b98e4a6753c474c4f4cfa8189ef18fa055c5b3a7f54a9c674a9c04e5  data/fightfx.sff
82cf3132b38be9b1f88487f58d37067e7d2c7dd5b19e2994618ddfe60ff26945  data/fight.snd
9303a0be5b229397865c6200fdc1bef526423d85996d5102ffea8b84bfaae1d0  readme.txt
SUMS
) || { echo "reference/mugen-1.1b1 missing or changed: run reference/fetch.sh"; exit 1; }
for f in common1.cns common.snd fightfx.air fightfx.sff fight.snd; do
  cp "$src/data/$f" "$out/"
done
cp "$src/readme.txt" "$out/mugen-readme.txt"
cat > "$out/NOTICE.txt" <<'TXT'
The files in this folder (common1.cns, common.snd, fightfx.air,
fightfx.sff, fight.snd) come unmodified from M.U.G.E.N 1.1 beta 1 by
Elecbyte (base motif sample content, Creative Commons Attribution-
NonCommercial 3.0; see mugen-readme.txt, which includes the M.U.G.E.N
license). They are not covered by rugen's MIT / Apache-2.0 license.
rugen is not affiliated with or endorsed by Elecbyte.

Put your own MUGEN data/ folder next to chars/ to use it instead.
TXT
ls -1 "$out"

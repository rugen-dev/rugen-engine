# Reference material

| Dir | What | Source |
|-----|------|--------|
| `mugen-1.1b1/` | Official M.U.G.E.N 1.1 Beta 1 (27 Jul 2013, Elecbyte). Binary, KFM, docs, sample stage, tools (sprmake2, sndmaker, sff2png). | archive.org item `mugen-1.1b1`, sha1 `8ab8e69877aea6f0cad1b9fa2970ba509638745f` |
| `mugen-docs/` | Official 1.1b1 HTML docs: `cns.html`, `trigger.html`, `sctrls.html`, `air.html`, `spr.html`, `bgs.html`, `fonts.html`, `storyboard.html`, `coordspace.html`, ... | mirrored from elecbyte.com/mugendocs-11b1/ |
| `Ikemen-GO/` | Ikemen GO source (Go). The de-facto semantic spec. | github ikemen-engine/Ikemen-GO, shallow clone |
| `ikemen-go-v1.0.0/` | Ikemen GO v1.0.0 linux build + content pack: `chars/kfm*`, stages (incl. `interactivestage`), data, screenpack. Reference runtime for frame-diff harness. | github release v1.0.0, sha256 verified |
| `dl/` | Raw archives. | |

Everything except this README and `fetch.sh` is gitignored. Run `./fetch.sh` to populate.

MUGEN 1.1b1 is closed-source freeware; there is no source to clone. Treat `mugen.exe` as a black box oracle (run under Wine) and Ikemen GO as the readable spec.

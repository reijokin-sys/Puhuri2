# Third-party notices

Puhuri is **AGPL-3.0-or-later** (the SPDX headers in the source files). This is
compatible with the one copyleft library it links, which is why the whole
application is distributed under those terms.

| What | Where it lives | Licence | Verified |
| --- | --- | --- | --- |
| **libsignal-protocol-c v2.3.3** (the OMEMO ratchet), commit `3a83a4f4…` | `vendor/libsignal-protocol-c/` (source), built by `tools/build-libsignal-c.sh`, linked **statically** into the phone binary | **GPL-3.0** | read from `vendor/libsignal-protocol-c/LICENSE` (2026-10-11) |
| protobuf-c 1.5.1 (the runtime, one C file) | `vendor/protobuf-c-1.5.1/` (source **and** `LICENSE`) | **BSD-2-Clause** | verified 2026-10-11: the vendored files are byte-identical to the 1.5.1 release tarball (`cmp`), whose sha256 is in `VENDOR.txt`, and its `LICENSE` came from the same tarball |
| `tokio-xmpp` 6.0 and the other Rust crates (247 packages in the lock file) | `daemon/Cargo.lock`, not vendored | per crate; **read, not assumed** - `cargo deny check licenses` passes under the policy in `daemon/deny.toml` | verified 2026-10-11: `licenses ok`, `advisories ok`, `bans ok`, `sources ok`; `tokio-xmpp` is MPL-2.0, the RustCrypto crates MIT/Apache-2.0, and the one copyleft-adjacent entry is `r-efi` (MIT **OR** Apache-2.0 **OR** LGPL-2.1-or-later - the permissive branch is taken) |
| **Puhuri's own theme modules** (`PuhuriPunainen`, `PuhuriVihrea`, `PuhuriPerus`) | `app/Lomiri/Components/Themes/`, shipped **inside the click**; made by `tools/sync-teemat.sh` from `../ubuntu-touch-teemat` | **LGPL-3.0-or-later** (Canonical Ltd.'s Lomiri theme files; the colours are the user's own teemat project's modifications) | 2026-10-16: the files keep Canonical's LGPL header, each `.qml` now also carries an SPDX line **and a dated note of our modification** (module/type/file names renamed), and the LGPL-3.0 text travels with them (`app/Lomiri/Components/Themes/LICENSE.LGPL-3.0`). LGPL-3 goes to GPL-3 (§3) and AGPL-3 §13 allows combining with GPL-3, so the AGPL application may carry them |
| Lomiri/Ubuntu Touch components | the phone's own system | their own licences | not our distribution |

**Consequences that are already true:** the application is open source, so
GPLv3 fits it; the phone binary is a combined work with libsignal-protocol-c
and is distributed under the AGPL/GPL terms, with the source available (this
repository).

**The licence work is done (2026-10-11):** the full licence texts are in place
(`LICENSE` is the AGPL-3.0 text, 661 lines, from gnu.org;
`vendor/libsignal-protocol-c/LICENSE` is the GPL-3.0 text and
`vendor/protobuf-c-1.5.1/LICENSE` the BSD-2-Clause one), the Rust crates have
been checked with a real tool run rather than this table, the vendored sources
were re-verified byte-for-byte against their upstreams, and the OpenStore's
requirement was read from the store itself. What is left is **not a licence
task**: publishing to the OpenStore needs a **public** source repository (the
project repositories are private by the workspace rule), which is the same
step the Glucoid project took.

**Re-checked 2026-10-16** (before the planned store release): `cargo deny
check` is `advisories ok, bans ok, licenses ok, sources ok`. The run found two
**rustls 0.23.45** advisories (`RUSTSEC-2026-0346` - `Debug` impls of client
session values could print TLS secrets - and the ECH-rejection one); both are
fixed in 0.23.46, so `cargo update -p rustls` was run (`0.23.45 -> 0.23.46`)
and the check passes. The same run is what made the theme modules' LGPL
position visible - they are the one third-party part that had been added
without a row here.

This file is a maintainer's note, not legal advice.

## The click's generated licence bundle

`tools/make-license-bundle.py` reads the actual `aarch64-unknown-linux-gnu`
OMEMO dependency graph, records every package's SPDX expression and upstream
authors/repository, collects the available licence/notice files, and adds the
full libsignal-protocol-c GPL-3.0, protobuf-c BSD-2-Clause and theme
LGPL-3.0 texts. It writes the deterministic `THIRD_PARTY_LICENSES.txt` that
is copied into the click next to `LICENSE` and this file.

`python3 tools/make-license-bundle.py --check` fails if the checked-in bundle
is stale. `tools/test-release-package.sh` compares all three files in the
built click with the source and verifies representative AGPL/GPL/LGPL/MPL/BSD
texts.

## How it is checked

The policy lives in `daemon/deny.toml` (allowed licences, no ignored
advisories, only crates.io as a source, all features - so the OMEMO-only
crates are in the graph too). Run:

```sh
cd daemon
cargo deny check licenses     # the licence policy
cargo deny check              # + advisories, bans, sources (needs the network)
```

Result on 2026-10-11 (cargo-deny 0.20.2): **`licenses ok`, `advisories ok`,
`bans ok`, `sources ok`** - no crate with an unknown or strong-copyleft
licence, no unignored advisory, nothing from a git or unknown registry. The
only noise is the multiple-version warning (two `r-efi` versions), which the
default `bans` policy reports as a warning.

The readable per-crate list is the `cargo metadata` summary at the bottom of
this file; it is the *listing*, while `cargo deny` is the *check*.

## The vendored sources were re-verified (2026-10-11)

Both vendored trees were checked again against their upstreams, not trusted
from the earlier note:

* `vendor/libsignal-protocol-c/` - a fresh `git clone --depth 1 --branch
  v2.3.3` of `signalapp/libsignal-protocol-c` (commit `3a83a4f4…`, the one
  `VENDOR.txt` names) and `diff -r` against our copy: **the only difference is
  our `VENDOR.txt`**. No local patch lives inside the vendored tree.
* `vendor/protobuf-c-1.5.1/` - the 1.5.1 release tarball (`build/libsignal-c/pb-c.tar.gz`,
  sha256 `20d1dc25…`, the value `VENDOR.txt` records) was unpacked and
  `cmp`-ed: `protobuf-c/protobuf-c.c`, `protobuf-c/protobuf-c.h` and `LICENSE`
  are **byte-identical** to the tarball.

One detail worth knowing: the build script also unpacks the whole protobuf-c
tarball into `build/libsignal-c/protobuf-c/` (91 files, tracked in the
repository because of the "artifacts belong in the repo" rule). It carries its
own `LICENSE`; it is a build input, not a second vendored copy, and
`vendor/protobuf-c-1.5.1/` is the one the notices point at.

## The OpenStore's own requirement (read from the store, 2026-10-11)

Read from the store itself, because there is no separate policy page:

* `https://open-store.io/about` says: *"The OpenStore encourages the apps
  published within to be open source, but also accepts proprietary apps"* and
  *"The apps available for download from the OpenStore have their own licensing
  and terms."* So **the store does not impose a licence** - it records the one
  the app declares.
* The upload form asks for a **licence picked from a fixed list**. Our own
  earlier submission work (the Glucoid project, recorded in the workspace
  `docs/CHANGELOG.md`) has that list: AGPLv3, Apache, BSD, CC\*, GFDL, GPLv2
  only, GPLv2 or later, GPLv3, LGPLv2, LGPLv3, MIT, MRR, PLR, OFL.
  **Puhuri picks `AGPLv3`.** The list has no "-or-later" variants, so the
  label is coarser than SPDX: the repository keeps `AGPL-3.0-or-later` in the
  files and the store shows `AGPLv3`, exactly as the Glucoid project chose the
  GPLv3 label for a `GPL-3.0-or-later` source.
* The public API shows that the licence is per app
  (`https://open-store.io/api/v4/apps/…` returns a `license` field, e.g.
  `"GNU LGPL v3"` for an existing app), so what Puhuri declares is what users
  will see.
* The submit form needs a **source-code link**, and the account email; the
  terms add an automated/manual review for security and privacy issues. The
  source link is the one thing Puhuri does not have yet: the repositories are
  private by the workspace rule, so a public repository (or the customary
  release tree, as in the Glucoid project) comes first.

## SPDX headers

All of our own source files carry `SPDX-License-Identifier:
AGPL-3.0-or-later` (Rust, QML, Python, shell, the Lua test-server config, the
systemd unit and `Cargo.toml`). The click's **declaration files that have no
comment syntax** - `app/manifest.json`, `app/push.json`,
`app/fi.reijo.puhuri2.apparmor`, `app/push.apparmor` - and `app/puhuri.desktop`
carry no header; they are part of the same AGPL-3.0-or-later work and are
covered by the repository `LICENSE`.

### Rust crate licences (from `cargo metadata`, 2026-10-11)

Regenerate with: `cd daemon && cargo metadata --format-version 1 | python3 -c ...` (see the project CHANGELOG). Summary:

* 138 crates: MIT OR Apache-2.0
* 37 crates: MIT
* 18 crates: Unicode-3.0
* 12 crates: Apache-2.0 OR MIT
* 8 crates: MIT/Apache-2.0
* 7 crates: MPL-2.0
* 3 crates: Apache-2.0 OR ISC OR MIT
* 3 crates: Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
* 2 crates: Unlicense OR MIT
* 2 crates: MIT OR Apache-2.0 OR LGPL-2.1-or-later
* 2 crates: ISC
* 2 crates: Unlicense/MIT
* 1 crates: ISC AND (Apache-2.0 OR ISC)
* 1 crates: ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)
* 1 crates: CC0-1.0 OR MIT-0 OR Apache-2.0
* 1 crates: Zlib
* 1 crates: (MIT OR Apache-2.0) AND Apache-2.0
* 1 crates: AGPL-3.0-or-later
* 1 crates: Apache-2.0 AND ISC
* 1 crates: Apache-2.0 OR BSL-1.0
* 1 crates: BSD-3-Clause
* 1 crates: Apache-2.0
* 1 crates: Zlib OR Apache-2.0 OR MIT
* 1 crates: (MIT OR Apache-2.0) AND Unicode-3.0
* 1 crates: CDLA-Permissive-2.0

Crates that are copyleft or state no licence (checked by name and version):
* puhurid 0.1.0 - AGPL-3.0-or-later
* r-efi 5.3.0 - MIT OR Apache-2.0 OR LGPL-2.1-or-later
* r-efi 6.0.0 - MIT OR Apache-2.0 OR LGPL-2.1-or-later

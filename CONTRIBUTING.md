# Contributing to plcc

Contributions are welcome. There is no CLA — you keep the copyright to your
work. All we ask is a sign-off certifying you have the right to contribute it.

## License

plcc is licensed under the [Mozilla Public License 2.0](LICENSE), with a
[compiler output exception](LICENSE-EXCEPTION).

Contributions are accepted under the same terms (inbound = outbound). New
source files should carry:

```rust
// SPDX-License-Identifier: MPL-2.0
```

MPL reciprocity is file-level. If you modify a plcc source file, that file
stays MPL — but building plcc into a larger product, proprietary or otherwise,
places no obligation on the rest of that product.

## Sign your commits (DCO)

Every commit needs a `Signed-off-by` line. Git adds it for you:

```bash
git commit -s -m "your message"
```

To sign off a commit you already made:

```bash
git commit --amend -s --no-edit
```

The sign-off certifies the Developer Certificate of Origin, version 1.1:

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

## Code provenance

plcc is a clean-room implementation. Do **not** copy code from GPL or LGPL
projects, including RuSTy, matiec, Beremiz, OpenPLC v3, or OpenPLC Editor.
`Autonomy-Logic/openplc-runtime` (OpenPLC v4) is MIT and may be used with
attribution — check the per-file headers, not just the repository badge, since
runtimes often vendor third-party sources under different terms.

IEC 61131-3 source files from external corpora are test inputs only. They are
fetched into the gitignored `tests/external/` directory and never committed.

## Before opening a PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

Tests must pass both in parallel and single-threaded:

```bash
cargo test --workspace -- --test-threads=1
```

If your change touches the parser, run the OSCAT conformance check — it holds
the parser to a failure rate under 5%:

```bash
just fetch-external
just test-oscat
```

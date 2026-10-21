# Statically linked language runtimes

These upstream notices were copied byte-for-byte from the compiler distributions
used for Puhuri 2 1.0.0. Versions and SHA-256 values are in provenance.json.
The generated THIRD_PARTY_LICENSES.txt includes the full texts in the click.

The Qt plugin's defined std::__1, __cxxabiv1 and _Unwind symbols identify the
LLVM libc++, libc++abi and libunwind runtime code linked by Zig. Their supplied
license texts specify Apache-2.0 WITH LLVM-exception and retain the historical
licenses/copyrights. Zig's runtime is under the accompanying MIT license.

The Rust distribution's library copyright report contains the full standard
library and third-party notices supplied for Rust libraries. The report lists
components of the library distribution; inclusion here does not assert that
every listed optional component is linked into Puhuri 2.

Upstreams: https://github.com/ziglang/zig and https://github.com/rust-lang/rust.
This adds notices for compiler runtimes omitted by earlier package audits; it
does not add or update a runtime library. The compiler executables and the
release-only binutils tool are not distributed with the application.

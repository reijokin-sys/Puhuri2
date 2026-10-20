// SPDX-License-Identifier: AGPL-3.0-or-later
//! Links the OMEMO ratchet.
//!
//! The ratchet is `libsignal-protocol-c`, the library the deployed OMEMO 0.3
//! clients themselves use (see `docs/DECISIONS.md`). The phone uses the static
//! aarch64 build made by `tools/build-libsignal-c.sh`; a host build (which is
//! how the OMEMO layer is developed and tested) uses the distribution's own
//! library.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_FEATURE_OMEMO").is_none() {
        return;
    }

    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch == "aarch64" {
        let prefix = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../build/libsignal-c/prefix");
        println!("cargo:rerun-if-changed={}/lib", prefix.display());
        println!("cargo:rustc-link-search=native={}/lib", prefix.display());
        println!("cargo:rustc-link-lib=static=signal-protocol-c");
        println!("cargo:rustc-link-lib=static=protobuf-c");
    } else {
        // The host build (this is how the OMEMO layer is tested) uses the
        // **vendored source** as well, built natively by
        // `tools/build-libsignal-c-host.sh`. Falling back to the packaged
        // library would mean the tests run a different build than the phone.
        let host = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../build/libsignal-c/host/prefix");
        if host.join("lib/libsignal-protocol-c.a").exists() {
            println!("cargo:rerun-if-changed={}/lib", host.display());
            println!("cargo:rustc-link-search=native={}/lib", host.display());
            println!("cargo:rustc-link-lib=static=signal-protocol-c");
            println!("cargo:rustc-link-lib=static=protobuf-c");
        } else if std::env::var_os("ALLOW_SYSTEM_SIGNAL").is_some() {
            // An explicit, conscious choice (a test run that wants to see how
            // the packaged library behaves). The default refuses: the whole
            // point of vendoring was that the tests and the phone do not end
            // up on different builds.
            println!("cargo:warning=ALLOW_SYSTEM_SIGNAL=1: käytetään järjestelmän libsignalia");
            println!("cargo:rustc-link-lib=dylib=signal-protocol-c");
            println!("cargo:rustc-link-lib=dylib=protobuf-c");
        } else {
            panic!(
                "build/libsignal-c/host puuttuu: aja tools/build-libsignal-c-host.sh \
                 (tai aseta ALLOW_SYSTEM_SIGNAL=1 käyttääksesi järjestelmän kirjastoa)"
            );
        }
    }
}

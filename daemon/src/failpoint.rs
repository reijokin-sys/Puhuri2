// SPDX-License-Identifier: AGPL-3.0-or-later
//! Nimetyt kaatumis- ja vikapisteet OMEMO:n turvatesteille.
//!
//! Testit (`src/engine/xmpp_crash_tests.rs`) tarvitsevat kaksi asiaa, joita
//! tuotantokoodi ei muuten tarjoa:
//!
//! * **kaatumisen** täsmälleen siinä kohdassa, jossa tila on kesken - siksi
//!   [`hit`] ja makro `failpoint!`, jotka kaatavat prosessin
//!   (`PUHURI_FAILPOINT=<nimi>`), ja
//! * **injektoidun virheen** ilman kaatumista - siksi [`fault`] ja makro
//!   `faultpoint!` (`PUHURI_FAULT=<nimi>`).
//!
//! Kumpikaan ei kuulu julkaisuun: ilman `failpoints`-featurea makrot kääntyvät
//! tyhjiksi, joten nimiä ei ole release-binäärissä lainkaan. Sen todistaa
//! `tools/failpoint-tarkistus.sh`.

/// Kaatopiste: jos `PUHURI_FAILPOINT` on tämä nimi, prosessi kaatuu heti.
///
/// `abort()` tuottaa SIGABRTin, jota testin vanhempi odottaa. Ennen kaatoa
/// core-tiedostot kytketään pois: muuten jokainen testiajo jättäisi
/// työhakemistoon muistivedoksen, jossa on avaimet.
#[cfg(feature = "failpoints")]
pub fn hit(name: &str) {
    if std::env::var("PUHURI_FAILPOINT").ok().as_deref() != Some(name) {
        return;
    }
    #[cfg(unix)]
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &limit);
    }
    eprintln!("puhurid: failpoint {name}: prosessi kaadetaan (PUHURI_FAILPOINT)");
    std::process::abort();
}

/// Ilman `failpoints`-featurea kaikilla nimillä tehdään sama: ei mitään.
///
/// Funktio on olemassa, jotta kutsukohdat eivät tarvitse `cfg`-attribuutteja;
/// makrot hoitavat sen, ettei nimi päädy käännökseen.
#[cfg(not(feature = "failpoints"))]
#[inline(always)]
pub fn hit(_name: &str) {}

/// Injektoitu vika: onko tämä vika päällä? (`PUHURI_FAULT=<nimi>`)
#[cfg(feature = "failpoints")]
pub fn fault(name: &str) -> bool {
    std::env::var("PUHURI_FAULT").ok().as_deref() == Some(name)
}

#[cfg(not(feature = "failpoints"))]
#[inline(always)]
pub fn fault(_name: &str) -> bool {
    false
}

/// Kaatopiste kutsujan kohdalle. Ilman `failpoints`-featurea argumentti ei
/// edes aukea, joten merkkijono ei ole binäärissä.
#[cfg(feature = "failpoints")]
#[macro_export]
macro_rules! failpoint {
    ($name:expr) => {
        $crate::failpoint::hit($name)
    };
}

#[cfg(not(feature = "failpoints"))]
#[macro_export]
macro_rules! failpoint {
    ($name:expr) => {};
}

/// Injektoitu vika kutsujan kohdalle; ilman featurea aina `false`.
#[cfg(feature = "failpoints")]
#[macro_export]
macro_rules! faultpoint {
    ($name:expr) => {
        $crate::failpoint::fault($name)
    };
}

#[cfg(not(feature = "failpoints"))]
#[macro_export]
macro_rules! faultpoint {
    ($name:expr) => {
        false
    };
}

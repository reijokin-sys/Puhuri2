// SPDX-License-Identifier: AGPL-3.0-or-later
//! The background service.
//!
//!     puhurid [--data-dir DIR] [--socket PATH] [--engine xmpp|fake] [--service-mode]
//!
//! Puhuri 2's confined UI launches it as a child process. It can also run by
//! hand for development and debugging.

use std::path::PathBuf;
use std::sync::Arc;

use puhurid::engine::{Engine, FakeEngineHolder};
use puhurid::server::Server;
use puhurid::store::Store;
use puhurid::{default_data_dir, default_socket_path};

fn main() {
    let mut data_dir = default_data_dir();
    let mut socket_path = default_socket_path();
    let mut engine_name = "xmpp".to_string();
    let mut service_mode = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--service-mode" => service_mode = true,
            "--data-dir" => {
                data_dir = PathBuf::from(args.next().unwrap_or_else(|| usage()));
            }
            "--socket" => {
                socket_path = PathBuf::from(args.next().unwrap_or_else(|| usage()));
            }
            "--engine" => {
                engine_name = args.next().unwrap_or_else(|| usage());
            }
            "--print-paths" => {
                println!("data={}", data_dir.display());
                println!("socket={}", socket_path.display());
                return;
            }
            "--version" => {
                println!("puhurid {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("tuntematon valitsin: {other}");
                usage();
            }
        }
    }

    std::fs::create_dir_all(&data_dir).expect("tietohakemiston luonti epäonnistui");
    // Hold ownership before opening SQLite or starting XMPP. The UI fallback
    // and systemd must never operate two writers on the same data directory.
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let _instance_lock = std::fs::OpenOptions::new()
        .read(true).write(true).create(true).truncate(false).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(data_dir.join("daemon.lock"))
        .expect("daemon-lukon avaus epäonnistui");
    if unsafe { libc::flock(_instance_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("puhurid: tietovarastolla on jo daemon; toista ei käynnistetä");
        std::process::exit(1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
    }

    if std::env::var_os("PUHURI_CLICK_WATCHDOG").is_some() {
        if let Ok(executable) = std::env::current_exe() {
            if let Some(root) = puhurid::click_current_root(&executable) {
                std::thread::Builder::new()
                    .name("puhuri-click-watchdog".to_string())
                    .spawn(move || loop {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        let current = std::fs::canonicalize(root.join("lib/aarch64-linux-gnu/bin/puhurid"));
                        if current.as_ref().ok() != Some(&executable) {
                            eprintln!("puhurid: click poistettu tai päivitetty; taustapalvelu lopetetaan");
                            std::process::exit(0);
                        }
                    })
                    .expect("click-watchdogin käynnistys epäonnistui");
            }
        }
    }

    let store = Arc::new(Store::open(&data_dir).expect("tietokannan avaus epäonnistui"));

    store.retire_push_bridge().expect("push-sillan käytöstäpoisto epäonnistui");
    // Offline-on-open is independent of the retired notification setup.
    store.set_setting_for_archive().expect("arkistovalinnan tallennus epäonnistui");
    store.set_poll_seconds(None).expect("yhteystilan tallennus epäonnistui");

    let (engine, fake): (Arc<dyn Engine>, Option<Arc<FakeEngineHolder>>) = match engine_name.as_str()
    {
        "xmpp" => (
            Arc::new(puhurid::engine::xmpp::XmppEngine::new(Arc::clone(&store))),
            None,
        ),
        "fake" => {
            let holder = Arc::new(FakeEngineHolder::new(Arc::clone(&store)));
            (holder.engine(), Some(holder))
        }
        other => {
            eprintln!("tuntematon moottori: {other} (valitse xmpp tai fake)");
            std::process::exit(2);
        }
    };

    // Log in again with the account the user last used, so a restart - or a
    // reboot - does not ask for the password over again. The account is
    // written when a login has really succeeded and removed by logging out.
    if let Some((jid, password)) = store.account() {
        match engine.connect(&jid, &password) {
            Ok(()) => eprintln!("puhurid: kirjaudutaan tallennetulla tilillä"),
            Err(error) => {
                eprintln!("puhurid: tallennettuun tiliin ei saatu yhteyttä: {error}")
            }
        }
    }

    let purge_marker = std::env::var_os("PUHURI_PURGE_REQUEST_DIR").map(|base| {
        PathBuf::from(base).join("purge-ui-dirs")
    });
    let exit_after_purge = std::env::var_os("PUHURI_RESTART_AFTER_PURGE").is_some();
    let server = Server::new(engine, fake, store, data_dir)
        .with_service_mode(service_mode)
        .with_purge_marker(purge_marker, exit_after_purge);
    if let Err(error) = server.run(&socket_path) {
        eprintln!("puhurid: palvelin pysähtyi: {error}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!(
        "käyttö: puhurid [--data-dir DIR] [--socket PATH] [--engine xmpp|fake] \
         [--service-mode] [--print-paths] [--version]"
    );
    std::process::exit(2)
}

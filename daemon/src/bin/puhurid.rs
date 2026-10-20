// SPDX-License-Identifier: AGPL-3.0-or-later
//! The background service.
//!
//!     puhurid [--data-dir DIR] [--socket PATH] [--engine xmpp|fake]
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

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
                        if !root.join("lib/aarch64-linux-gnu/bin/puhurid").exists() {
                            eprintln!("puhurid: click poistettu; taustapalvelu lopetetaan");
                            std::process::exit(0);
                        }
                    })
                    .expect("click-watchdogin käynnistys epäonnistui");
            }
        }
    }

    let store = Arc::new(Store::open(&data_dir).expect("tietokannan avaus epäonnistui"));

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
        .with_purge_marker(purge_marker, exit_after_purge);
    if let Err(error) = server.run(&socket_path) {
        eprintln!("puhurid: palvelin pysähtyi: {error}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!(
        "käyttö: puhurid [--data-dir DIR] [--socket PATH] [--engine xmpp|fake] \
         [--print-paths] [--version]"
    );
    std::process::exit(2)
}

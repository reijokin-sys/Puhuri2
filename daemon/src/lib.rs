// Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
// Copyright (C) 2026 Reijo Kinnunen
//
// SPDX-License-Identifier: AGPL-3.0-or-later
//
//! The background service. It owns the XMPP connection and the message store,
//! and serves the user interface (and `puhuri-ctl`) over a unix socket with
//! one JSON object per line.
//!
//! The XMPP protocol itself is not written here: the [`engine::xmpp`] module
//! drives `tokio-xmpp`, and the message store is SQLite through `rusqlite`.
//! Only the bridge - the socket protocol, the engine trait and the user
//! interface - is ours.

pub mod db;
pub mod engine;
pub mod failpoint;
pub mod protocol;
pub mod server;
pub mod store;

/// The application id, used for the runtime directory and the data directory.
pub const APP_ID: &str = "fi.reijo.puhuri2";

/// The stable click symlink when this executable came from an installed click.
pub fn click_current_root(executable: &std::path::Path) -> Option<std::path::PathBuf> {
    for ancestor in executable.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) == Some(APP_ID) {
            return Some(ancestor.join("current"));
        }
    }
    None
}

/// Where the data lives (`~/.local/share/fi.reijo.puhuri2` by default).
pub fn default_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    if base.ends_with(APP_ID) {
        base
    } else {
        base.join(APP_ID)
    }
}

/// Where the click's language and theme choices live.
pub fn default_config_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    if base.ends_with(APP_ID) {
        base
    } else {
        base.join(APP_ID)
    }
}

/// Where Qt, Content Hub and the click keep regenerable cache data.
pub fn default_cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    if base.ends_with(APP_ID) {
        base
    } else {
        base.join(APP_ID)
    }
}

/// Where the socket lives (`/run/user/<uid>/fi.reijo.puhuri2/socket`).
pub fn default_socket_path() -> std::path::PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    if runtime.ends_with(APP_ID) {
        runtime.join("socket")
    } else {
        runtime.join(APP_ID).join("socket")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_click_executable_maps_to_its_current_symlink() {
        let executable = std::path::Path::new(
            "/opt/click.ubuntu.com/fi.reijo.puhuri2/0.1.0/lib/aarch64-linux-gnu/bin/puhurid",
        );
        assert_eq!(
            click_current_root(executable),
            Some(std::path::PathBuf::from(
                "/opt/click.ubuntu.com/fi.reijo.puhuri2/current"
            ))
        );
        assert_eq!(click_current_root(std::path::Path::new("/tmp/puhurid")), None);
    }
}

// SPDX-License-Identifier: AGPL-3.0-or-later
//! One XMPP/OMEMO owner across the UI and short-lived push process.
use std::{fs::{File, OpenOptions}, os::fd::AsRawFd, os::unix::fs::OpenOptionsExt, path::Path, time::{Duration, Instant}};

pub struct SessionLock(File);
impl SessionLock {
    pub fn acquire(data: &Path, budget: Duration) -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(data.join("xmpp.lock"))?;
        let started = Instant::now();
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 { return Ok(Self(file)); }
            let error=std::io::Error::last_os_error();
            if error.kind()!=std::io::ErrorKind::WouldBlock || started.elapsed()>=budget { return Err(error); }
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}
impl Drop for SessionLock {
    fn drop(&mut self) { unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_one_process_handle_can_own_the_network_and_ratchet() {
        let dir=std::env::temp_dir().join(format!("puhuri-session-lock-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first=SessionLock::acquire(&dir,Duration::ZERO).unwrap();
        assert!(SessionLock::acquire(&dir,Duration::ZERO).is_err());
        drop(first);
        assert!(SessionLock::acquire(&dir,Duration::ZERO).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

//! Cross-process playback queue.
//!
//! Every MCP client (each Claude session, each agent host) runs its own
//! `local-voice serve`, and the CLI is yet another process, so an in-process
//! queue cannot stop two of them from talking over each other. This module
//! serialises playback across all local-voice processes of the current user
//! with a strict first-come-first-served ticket queue on disk:
//!
//! * a player takes a ticket (a file named `<nanos>-<pid>` in a per-user
//!   temp directory, created atomically);
//! * it waits until its ticket is the lexicographically smallest one, i.e.
//!   everyone who arrived earlier has finished;
//! * dropping the [`PlaybackSlot`] removes the ticket and lets the next
//!   player in.
//!
//! Tickets of processes that died (crash, kill) are detected by checking
//! whether the pid is still alive and are removed by whoever is waiting, so a
//! crash can never wedge the queue.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often a waiting player re-checks the queue.
const POLL: Duration = Duration::from_millis(25);

/// A held position at the head of the playback queue. Drop it to let the
/// next player speak.
pub struct PlaybackSlot {
    ticket: PathBuf,
}

impl Drop for PlaybackSlot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.ticket);
    }
}

/// Per-user queue directory.
fn queue_dir() -> PathBuf {
    std::env::temp_dir().join("local-voice-playback-queue")
}

/// Block until this process is first in line to play audio, then return the
/// slot. Never fails: if the queue directory cannot be used (read-only temp
/// dir, exotic sandbox) playback proceeds unserialised, which is the
/// pre-existing behaviour.
pub fn acquire() -> Option<PlaybackSlot> {
    let dir = queue_dir();
    if fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let ticket = create_ticket(&dir)?;
    let my_name = ticket.file_name()?.to_os_string();
    loop {
        match head_of_queue(&dir) {
            Some(head) if head == my_name => return Some(PlaybackSlot { ticket }),
            Some(_) => thread::sleep(POLL),
            // Our own ticket vanished (temp dir cleaned?): re-issue it.
            None => {
                let _ = fs::remove_file(&ticket);
                return acquire();
            }
        }
    }
}

/// Atomically create a ticket named `<nanos since epoch>-<pid>`; the name
/// orders tickets by arrival time.
fn create_ticket(dir: &Path) -> Option<PathBuf> {
    let pid = std::process::id();
    loop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let path = dir.join(format!("{nanos:020}-{pid}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Some(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
}

/// Name of the oldest live ticket, removing dead ones on the way.
fn head_of_queue(dir: &Path) -> Option<std::ffi::OsString> {
    let mut names: Vec<std::ffi::OsString> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    names.sort();
    for name in names {
        let alive = ticket_pid(&name).is_some_and(pid_alive);
        if alive {
            return Some(name);
        }
        let _ = fs::remove_file(dir.join(&name));
    }
    None
}

fn ticket_pid(name: &std::ffi::OsStr) -> Option<u32> {
    name.to_str()?.rsplit_once('-')?.1.parse().ok()
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // kill(pid, 0) probes existence without sending a signal. EPERM means
    // the process exists but belongs to someone else: still alive.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // Access denied means it exists; not-found means it is gone.
            return std::io::Error::last_os_error().raw_os_error() == Some(5);
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE.0 as u32
    }
}

#[cfg(not(any(unix, windows)))]
fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn slots_are_served_in_arrival_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        // Hold the head so the others queue up behind it.
        let first = acquire().expect("queue usable");
        let mut handles = Vec::new();
        for i in 0..3 {
            let order = Arc::clone(&order);
            handles.push(thread::spawn(move || {
                // Stagger arrivals so ticket timestamps are strictly ordered.
                thread::sleep(Duration::from_millis(30 * i as u64));
                let _slot = acquire().expect("queue usable");
                order.lock().unwrap().push(i);
                thread::sleep(Duration::from_millis(40));
            }));
        }
        thread::sleep(Duration::from_millis(150));
        assert!(
            order.lock().unwrap().is_empty(),
            "nobody may play while the head is held"
        );
        drop(first);
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn dead_process_tickets_are_skipped() {
        let dir = queue_dir();
        fs::create_dir_all(&dir).unwrap();
        // A ticket that sorts first but belongs to a pid that cannot exist.
        let stale = dir.join("00000000000000000000-4000000000");
        fs::write(&stale, b"").unwrap();
        let slot = acquire().expect("queue usable");
        assert!(!stale.exists(), "stale ticket must be removed");
        drop(slot);
    }

    #[test]
    fn ticket_pid_parses() {
        assert_eq!(
            ticket_pid(std::ffi::OsStr::new("00000000000000000042-1234")),
            Some(1234)
        );
        assert_eq!(ticket_pid(std::ffi::OsStr::new("garbage")), None);
    }
}

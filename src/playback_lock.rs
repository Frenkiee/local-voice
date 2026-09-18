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
//!
//! A holder that is alive but stuck (its output device vanished and playback
//! never finishes) is caught by a deadline: before playing an item the holder
//! stamps its ticket with the time by which it promises to be done
//! ([`PlaybackSlot::promise_done_within`]). Waiters treat a ticket whose
//! promised deadline has passed as abandoned and remove it. A ticket without a
//! stamp is a process still waiting in line, or one that has not started its
//! first item yet; those are only ever judged by pid liveness.

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

impl PlaybackSlot {
    /// Promise the other processes to be done within `budget`. Call it before
    /// every item so the deadline always covers what is actually playing; a
    /// waiter that finds the deadline in the past assumes this holder is
    /// stuck and takes the queue over.
    pub fn promise_done_within(&self, budget: Duration) {
        let deadline = SystemTime::now()
            .checked_add(budget)
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        if let Some(deadline) = deadline {
            let _ = fs::write(&self.ticket, deadline.as_nanos().to_string());
        }
    }
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

/// Name of the oldest live ticket, removing dead and abandoned ones on the
/// way.
fn head_of_queue(dir: &Path) -> Option<std::ffi::OsString> {
    let mut names: Vec<std::ffi::OsString> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let alive = ticket_pid(&name).is_some_and(pid_alive);
        if alive && !ticket_expired(&path) {
            return Some(name);
        }
        let _ = fs::remove_file(path);
    }
    None
}

/// True when the ticket carries a promised deadline that has passed.
fn ticket_expired(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(deadline) = contents.trim().parse::<u128>() else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    now > deadline
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
    fn expired_ticket_of_live_process_is_skipped() {
        let dir = queue_dir();
        fs::create_dir_all(&dir).unwrap();
        // Our own pid is certainly alive, but the promised deadline is long
        // gone: a stuck holder.
        let stuck = dir.join(format!("00000000000000000001-{}", std::process::id()));
        fs::write(&stuck, b"1").unwrap();
        let slot = acquire().expect("queue usable");
        assert!(!stuck.exists(), "abandoned ticket must be removed");
        drop(slot);
    }

    #[test]
    fn promised_deadline_in_future_is_honoured() {
        let dir = queue_dir();
        fs::create_dir_all(&dir).unwrap();
        let slot = acquire().expect("queue usable");
        slot.promise_done_within(Duration::from_secs(60));
        assert!(!ticket_expired(&slot.ticket));
        slot.promise_done_within(Duration::ZERO);
        thread::sleep(Duration::from_millis(2));
        assert!(ticket_expired(&slot.ticket));
        assert!(!ticket_expired(Path::new("/nonexistent/ticket")));
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

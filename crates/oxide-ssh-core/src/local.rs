//! Local shell sessions: a platform shell on a PTY, no SSH involved.

use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use async_channel::{Receiver, Sender, TrySendError};
use bytes::Bytes;
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, PtySize, native_pty_system};

use crate::{
    model::{SessionId, TerminalSize},
    session::{
        EVENT_QUEUE_CAPACITY, INPUT_QUEUE_CAPACITY, SendInputError, SessionError, SessionEvent,
        SessionState,
    },
};

/// Spawns the platform default shell on a local PTY.
///
/// Shell selection: `$SHELL` (Unix) / `%COMSPEC%` (Windows), falling back to
/// `/bin/sh` / `cmd.exe`. The shell starts in the user's home directory.
pub fn spawn_local_shell(initial_size: TerminalSize) -> Result<LocalSessionHandle, SessionError> {
    if !initial_size.is_valid() {
        return Err(SessionError::InvalidProfile);
    }
    let pty = native_pty_system();
    let pair = pty
        .openpty(pty_size(initial_size))
        .map_err(|_| SessionError::LocalShellFailed)?;
    let mut command = CommandBuilder::new(default_shell());
    if let Some(home) = home_dir() {
        command.cwd(home);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| SessionError::LocalShellFailed)?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| SessionError::LocalShellFailed)?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|_| SessionError::LocalShellFailed)?;
    let killer = child.clone_killer();

    let id = SessionId::new();
    let (event_sender, event_receiver) = async_channel::bounded(EVENT_QUEUE_CAPACITY);
    let (input_sender, input_receiver) = async_channel::bounded::<Bytes>(INPUT_QUEUE_CAPACITY);
    let (resize_sender, resize_receiver) = mpsc::channel::<TerminalSize>();

    // Reader: PTY output -> session events.
    let reader_events = event_sender.clone();
    thread::Builder::new()
        .name("oxide-ssh-local-read".into())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if reader_events
                            .send_blocking(SessionEvent::Output(Bytes::copy_from_slice(
                                &buffer[..n],
                            )))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|_| SessionError::LocalShellFailed)?;

    // Writer: session input -> PTY. Ends when the handle closes the channel.
    thread::Builder::new()
        .name("oxide-ssh-local-write".into())
        .spawn(move || {
            while let Ok(bytes) = input_receiver.recv_blocking() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| SessionError::LocalShellFailed)?;

    // Resize: applies the latest queued size, draining stale ones.
    thread::Builder::new()
        .name("oxide-ssh-local-resize".into())
        .spawn(move || {
            while let Ok(size) = resize_receiver.recv() {
                let mut latest = size;
                while let Ok(newer) = resize_receiver.try_recv() {
                    latest = newer;
                }
                let _ = pair.master.resize(pty_size(latest));
            }
        })
        .map_err(|_| SessionError::LocalShellFailed)?;

    // Waiter: child exit -> Exited + Disconnected, mirroring the SSH flow.
    let waiter_events = event_sender.clone();
    thread::Builder::new()
        .name("oxide-ssh-local-wait".into())
        .spawn(move || {
            let status = child
                .wait()
                .ok()
                .map(|status: ExitStatus| status.exit_code());
            let _ = waiter_events.send_blocking(SessionEvent::Exited { status });
            let _ =
                waiter_events.send_blocking(SessionEvent::StateChanged(SessionState::Disconnected));
        })
        .map_err(|_| SessionError::LocalShellFailed)?;

    let _ = event_sender.send_blocking(SessionEvent::StateChanged(SessionState::Connected));

    Ok(LocalSessionHandle {
        id,
        events: Some(event_receiver),
        input: input_sender,
        resize: resize_sender,
        killer: Mutex::new(killer),
        disconnect_started: AtomicBool::new(false),
    })
}

pub struct LocalSessionHandle {
    id: SessionId,
    events: Option<Receiver<SessionEvent>>,
    input: Sender<Bytes>,
    resize: mpsc::Sender<TerminalSize>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    disconnect_started: AtomicBool,
}

impl LocalSessionHandle {
    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn take_events(&mut self) -> Option<Receiver<SessionEvent>> {
        self.events.take()
    }

    pub fn try_send_input(&self, bytes: Bytes) -> Result<(), SendInputError> {
        self.input.try_send(bytes).map_err(|error| match error {
            TrySendError::Full(_) => SendInputError::QueueFull,
            TrySendError::Closed(_) => SendInputError::Closed,
        })
    }

    pub fn resize(&self, size: TerminalSize) {
        if size.is_valid() {
            // The resize thread drains to the latest value; a full queue only
            // means a resize is already pending.
            let _ = self.resize.send(size);
        }
    }

    pub fn disconnect(&self) {
        if self.disconnect_started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.input.close();
        let _ = self.killer.lock().kill();
    }
}

impl Drop for LocalSessionHandle {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows.min(u16::MAX as u32) as u16,
        cols: size.columns.min(u16::MAX as u32) as u16,
        pixel_width: size.pixel_width.min(u16::MAX as u32) as u16,
        pixel_height: size.pixel_height.min(u16::MAX as u32) as u16,
    }
}

fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
}

fn home_dir() -> Option<String> {
    #[cfg(unix)]
    {
        std::env::var("HOME").ok()
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn size() -> TerminalSize {
        TerminalSize {
            columns: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 480,
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_shell_echoes_and_exits() {
        let mut handle = spawn_local_shell(size()).expect("spawn /bin/sh");
        let events = handle.take_events().expect("events");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        // Wait for Connected, then drive the shell.
        loop {
            match events.recv_blocking().expect("event before deadline") {
                SessionEvent::StateChanged(SessionState::Connected) => break,
                _ => assert!(std::time::Instant::now() < deadline, "timed out"),
            }
        }
        handle
            .try_send_input(Bytes::from_static(b"echo oxide-marker-42; exit\n"))
            .expect("input");
        let mut saw_marker = false;
        let mut exit_status = None;
        while std::time::Instant::now() < deadline && (!saw_marker || exit_status.is_none()) {
            match events.try_recv() {
                Ok(SessionEvent::Output(bytes)) => {
                    if bytes.windows(15).any(|w| w == b"oxide-marker-42") {
                        saw_marker = true;
                    }
                }
                Ok(SessionEvent::Exited { status }) => exit_status = Some(status),
                Ok(_) | Err(async_channel::TryRecvError::Empty) => {
                    std::thread::yield_now();
                }
                Err(async_channel::TryRecvError::Closed) => break,
            }
        }
        assert!(saw_marker, "shell output never arrived");
        assert_eq!(exit_status, Some(Some(0)));
    }
}

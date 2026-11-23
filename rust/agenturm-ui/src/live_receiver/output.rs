//! Deadline-bound Linux terminal output for the protected receiver viewer.

use super::{FrameGate, RenderOutput};
use crossterm::cursor::{Hide, Show};
use crossterm::style::{Attribute, ResetColor, SetAttribute};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{Mode, OFlags, fstat, open};
use rustix::io::Errno;
use std::cell::RefCell;
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const FRAME_TIME: Duration = Duration::from_millis(300);
const POLL_TIME: Duration = Duration::from_millis(10);
const MAX_CHUNK: usize = 512;

struct Phase {
    gate: Option<FrameGate>,
    deadline: Instant,
}

impl Phase {
    fn start(gate: Option<FrameGate>) -> Self {
        let deadline = Instant::now() + FRAME_TIME;
        let deadline = gate
            .as_ref()
            .map_or(deadline, |gate| deadline.min(gate.expires));
        Self { gate, deadline }
    }

    fn remaining(&self) -> io::Result<Duration> {
        if self
            .gate
            .as_ref()
            .is_some_and(|gate| !gate.active.load(Ordering::Acquire))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Private Review gate closed",
            ));
        }
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "Terminal frame deadline reached")
            })
    }
}

struct BoundedWriter {
    fd: OwnedFd,
    phase: Rc<RefCell<Phase>>,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let bytes = &bytes[..bytes.len().min(MAX_CHUNK)];
        loop {
            let remaining = self.phase.borrow().remaining()?;
            match rustix::io::write(&self.fd, bytes) {
                Ok(length) => return Ok(length),
                Err(Errno::INTR) => continue,
                Err(Errno::AGAIN) => {
                    let timeout = Timespec::try_from(remaining.min(POLL_TIME))
                        .expect("bounded poll interval");
                    let mut fds = [PollFd::new(&self.fd, PollFlags::OUT)];
                    match poll(&mut fds, Some(&timeout)) {
                        Ok(_) | Err(Errno::INTR) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.phase.borrow().remaining().map(|_| ())
    }
}

pub(super) fn open_stdout() -> io::Result<OwnedFd> {
    let stdout = io::stdout();
    if !rustix::termios::isatty(&stdout) {
        return Err(io::Error::other("Review requires terminal stdout"));
    }
    let original = fstat(&stdout)?;
    // Opening the kernel descriptor path gives the writer independent flags.
    // Duplicating stdout and setting O_NONBLOCK would change shared flags.
    let fd = open(
        "/proc/self/fd/1",
        OFlags::WRONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let opened = fstat(&fd)?;
    if (original.st_dev, original.st_ino, original.st_rdev)
        != (opened.st_dev, opened.st_ino, opened.st_rdev)
        || !rustix::termios::isatty(&fd)
    {
        return Err(io::Error::other("Review terminal identity changed"));
    }
    Ok(fd)
}

pub(super) struct BoundedTerminal {
    terminal: Terminal<CrosstermBackend<BoundedWriter>>,
    phase: Rc<RefCell<Phase>>,
    raw: bool,
}

impl BoundedTerminal {
    pub(super) fn new(fd: OwnedFd) -> io::Result<Self> {
        let phase = Rc::new(RefCell::new(Phase::start(None)));
        let backend = CrosstermBackend::new(BoundedWriter {
            fd,
            phase: Rc::clone(&phase),
        });
        let mut output = Self {
            terminal: Terminal::new(backend)?,
            phase,
            raw: false,
        };
        terminal::enable_raw_mode()?;
        output.raw = true;
        *output.phase.borrow_mut() = Phase::start(None);
        crossterm::execute!(output.terminal.backend_mut(), EnterAlternateScreen, Hide)?;
        Ok(output)
    }
}

impl RenderOutput for BoundedTerminal {
    fn draw_frame(
        &mut self,
        gate: Option<FrameGate>,
        draw: impl FnOnce(&mut Frame),
    ) -> io::Result<()> {
        *self.phase.borrow_mut() = Phase::start(gate);
        self.terminal.draw(draw).map(|_| ())
    }

    fn redraw_metadata(&mut self, draw: impl FnOnce(&mut Frame)) -> io::Result<()> {
        *self.phase.borrow_mut() = Phase::start(None);
        // A refused write can have left a partial terminal control sequence.
        // Cancel it, reset styling and invalidate the whole diff buffer.
        self.terminal.backend_mut().write_all(b"\x18")?;
        crossterm::execute!(
            self.terminal.backend_mut(),
            ResetColor,
            SetAttribute(Attribute::Reset)
        )?;
        self.terminal.clear()?;
        self.terminal.draw(draw).map(|_| ())
    }
}

impl Drop for BoundedTerminal {
    fn drop(&mut self) {
        if self.raw {
            *self.phase.borrow_mut() = Phase::start(None);
            let backend = self.terminal.backend_mut();
            let _ = backend.write_all(b"\x18");
            let _ = crossterm::execute!(backend, ResetColor, Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::pipe::{PipeFlags, pipe_with};
    use std::sync::{Arc, atomic::AtomicBool};

    #[test]
    fn backpressured_writer_stops_without_drain_and_never_resumes_private_chunks() {
        let (reader, output) = pipe_with(PipeFlags::NONBLOCK | PipeFlags::CLOEXEC).unwrap();
        let filler = [b'p'; 512];
        loop {
            match rustix::io::write(&output, &filler) {
                Ok(size) => assert_eq!(size, filler.len()),
                Err(Errno::AGAIN) => break,
                Err(error) => panic!("cannot fill fixture pipe: {error}"),
            }
        }
        let deadline_phase = Rc::new(RefCell::new(Phase {
            gate: None,
            deadline: Instant::now() + Duration::from_millis(20),
        }));
        let mut timeout = BoundedWriter {
            fd: output,
            phase: deadline_phase,
        };
        assert_eq!(
            timeout
                .write_all(b"private-frame-timeout")
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let output = timeout.fd;
        let active = Arc::new(AtomicBool::new(true));
        let worker_gate = Arc::clone(&active);
        let (begun, start) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert_eq!(rustix::io::write(&output, b"x"), Err(Errno::AGAIN));
            let phase = Rc::new(RefCell::new(Phase::start(Some(FrameGate {
                active: worker_gate,
                expires: Instant::now() + Duration::from_secs(25),
            }))));
            let mut writer = BoundedWriter { fd: output, phase };
            begun.send(()).unwrap();
            let result = writer.write_all(b"private-review-never-enqueued");
            (result, writer.fd)
        });
        // Only the authority flag changes. The pipe stays full until the
        // writer stops, proving revocation does not wait for terminal I/O.
        start.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!worker.is_finished());
        active.store(false, Ordering::Release);
        let (result, output) = worker.join().unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        let mut received = [0_u8; 2048];
        loop {
            match rustix::io::read(&reader, &mut received) {
                Ok(0) | Err(Errno::AGAIN) => break,
                Ok(length) => assert!(received[..length].iter().all(|&byte| byte == b'p')),
                Err(error) => panic!("cannot drain fixture pipe: {error}"),
            }
        }
        let phase = Rc::new(RefCell::new(Phase::start(Some(FrameGate {
            active,
            expires: Instant::now() + Duration::from_secs(25),
        }))));
        let mut writer = BoundedWriter { fd: output, phase };
        assert_eq!(
            writer
                .write_all(b"private-review-after-revoke")
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(rustix::io::read(&reader, &mut received), Err(Errno::AGAIN));
    }
}

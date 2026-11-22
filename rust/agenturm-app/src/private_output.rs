//! Deadline-aware stdout delivery for the CLI's private evidence commands.

use std::io::{self, Write};

/// Runs a CLI operation with unbuffered stdout delivery on Unix. The output
/// window starts before the operation requests a private proof. The consumer
/// must return promptly after a write error and must not retain private bytes
/// for later publication. Use the supplied writer exclusively for stdout
/// inside the operation. The deadline bounds nonblocking pipe and terminal
/// waits. A regular-file write can still block inside the kernel.
pub fn with_private_stdout<T, E: From<String>>(
    consume: impl FnOnce(&mut dyn Write) -> Result<T, E>,
) -> Result<T, E> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        use std::time::Duration;

        let stdout = io::stdout().lock();
        // Leave headroom before the broker's 32-second acknowledgement
        // timeout, which starts only after it sends the proof.
        let mut output = unix::DeadlineWriter::new(stdout.as_fd(), Duration::from_secs(25))
            .map_err(|_| String::from("Could not prepare private stdout"))?;
        let result = consume(&mut output);
        let restored = output
            .restore()
            .map_err(|_| String::from("Could not restore private stdout"));
        result.and_then(|value| restored.map(|()| value).map_err(E::from))
    }
    #[cfg(not(unix))]
    consume(&mut io::stdout().lock())
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::fd::{AsRawFd, BorrowedFd};
    use std::time::{Duration, Instant};

    pub(super) struct DeadlineWriter<'a> {
        fd: BorrowedFd<'a>,
        original_flags: Option<libc::c_int>,
        deadline: Instant,
    }

    impl<'a> DeadlineWriter<'a> {
        pub(super) fn new(fd: BorrowedFd<'a>, budget: Duration) -> io::Result<Self> {
            let deadline = Instant::now() + budget;
            let flags = status_flags(fd)?;
            let original_flags = if flags & libc::O_NONBLOCK == 0 {
                set_status_flags(fd, flags | libc::O_NONBLOCK)?;
                Some(flags)
            } else {
                None
            };
            Ok(Self {
                fd,
                original_flags,
                deadline,
            })
        }

        pub(super) fn restore(&mut self) -> io::Result<()> {
            if let Some(flags) = self.original_flags {
                set_status_flags(self.fd, flags)?;
                self.original_flags = None;
            }
            Ok(())
        }

        fn remaining(&self) -> io::Result<Duration> {
            self.deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Private output expired"))
        }

        fn wait_writable(&self) -> io::Result<()> {
            let mut ready = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            loop {
                let remaining_ms = self.remaining()?.as_millis().clamp(1, i32::MAX as u128);
                // SAFETY: ready is one initialized pollfd and its FD stays
                // valid for the lifetime of the borrowed stdout lock.
                let result = unsafe { libc::poll(&mut ready, 1, remaining_ms as i32) };
                if result > 0 {
                    return Ok(());
                }
                if result < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
        }
    }

    impl Write for DeadlineWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            loop {
                self.remaining()?;
                match write_unbuffered(self.fd, bytes) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.wait_writable()?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    result => return result,
                }
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            // Writes go directly to the FD. No stream buffer can resume a
            // private write after this writer expires or restores FD flags.
            self.remaining().map(|_| ())
        }
    }

    impl Drop for DeadlineWriter<'_> {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }

    fn status_flags(fd: BorrowedFd<'_>) -> io::Result<libc::c_int> {
        // SAFETY: F_GETFL takes no third argument and fd is live.
        retry_fcntl(|| unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) })
    }

    fn set_status_flags(fd: BorrowedFd<'_>, flags: libc::c_int) -> io::Result<()> {
        // SAFETY: fd is live and F_SETFL takes integer status flags.
        retry_fcntl(|| unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags) }).map(|_| ())
    }

    fn retry_fcntl(mut operation: impl FnMut() -> libc::c_int) -> io::Result<libc::c_int> {
        loop {
            let result = operation();
            if result >= 0 {
                return Ok(result);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn write_unbuffered(fd: BorrowedFd<'_>, bytes: &[u8]) -> io::Result<usize> {
        // SAFETY: fd is live and bytes remains readable during this write.
        let count = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Read;
        use std::os::fd::AsFd;

        #[test]
        fn an_unread_full_pipe_expires_without_late_bytes_and_restores_flags() {
            let (mut read_end, write_end) = io::pipe().unwrap();
            let fd = write_end.as_fd();
            let original = status_flags(fd).unwrap();
            let mut fill = DeadlineWriter::new(fd, Duration::from_secs(1)).unwrap();
            let mut filled = 0;
            for filler in [&[b'x'; 4096][..], &[b'x'][..]] {
                loop {
                    match write_unbuffered(fd, filler) {
                        Ok(count) => filled += count,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("could not fill pipe: {error}"),
                    }
                }
            }
            fill.restore().unwrap();
            let mut output = DeadlineWriter::new(fd, Duration::from_millis(25)).unwrap();
            assert_ne!(status_flags(fd).unwrap() & libc::O_NONBLOCK, 0);
            let private = b"fixture-private-output-must-not-escape";
            let error = output.write_all(private).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert_eq!(error.to_string(), "Private output expired");

            let mut contents = vec![0; filled];
            read_end.read_exact(&mut contents).unwrap();
            assert!(contents.iter().all(|byte| *byte == b'x'));
            assert_eq!(
                output.write_all(private).unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            drop(output);
            drop(fill);
            assert_eq!(status_flags(fd).unwrap(), original);
            drop(write_end);
            assert_eq!(read_end.read_to_end(&mut contents).unwrap(), 0);
        }

        #[test]
        fn delivery_and_unwinding_both_restore_the_descriptor() {
            let (mut read_end, write_end) = io::pipe().unwrap();
            let fd = write_end.as_fd();
            let original = status_flags(fd).unwrap();
            {
                let mut output = DeadlineWriter::new(fd, Duration::from_secs(1)).unwrap();
                output.write_all(b"fixture\n").unwrap();
                output.flush().unwrap();
                output.restore().unwrap();
                assert_eq!(status_flags(fd).unwrap(), original);
            }
            let mut published = [0; 8];
            read_end.read_exact(&mut published).unwrap();
            assert_eq!(&published, b"fixture\n");

            let unwind = std::panic::catch_unwind(|| {
                let _output = DeadlineWriter::new(fd, Duration::from_secs(1)).unwrap();
                assert_ne!(status_flags(fd).unwrap() & libc::O_NONBLOCK, 0);
                panic!("test unwind");
            });
            assert!(unwind.is_err());
            assert_eq!(status_flags(fd).unwrap(), original);
        }
    }
}

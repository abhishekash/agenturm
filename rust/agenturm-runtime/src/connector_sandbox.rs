//! Linux x86-64 launch profile for static, credential-free connectors.
//!
//! Landlock limits executable file access to one inode. A seccomp allowlist
//! permits stdio and computation but excludes file opens, sockets, process
//! creation and cross-process operations. Both restrictions are inherited
//! across exec and are installed before any connector instruction runs.

use std::ffi::{CStr, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command};

const FS_EXECUTE: u64 = 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;
const PATH_BENEATH: libc::c_int = 1;
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

#[repr(C, packed)]
struct PathBeneath {
    allowed_access: u64,
    parent_fd: libc::c_int,
}

pub(crate) struct Stage(PathBuf);

impl Stage {
    pub(crate) fn new() -> io::Result<Self> {
        let mut name = *b"/tmp/agenturm-connector-sandbox.XXXXXXXX\0";
        // SAFETY: mkdtemp updates the writable, terminated template in place.
        if unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        let name = CStr::from_bytes_until_nul(&name).expect("terminated mkdtemp template");
        Ok(Self(PathBuf::from(OsStr::from_bytes(name.to_bytes()))))
    }

    pub(crate) fn copy(&self, source: &File, max_bytes: u64) -> io::Result<File> {
        let executable = self.0.join("connector");
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o400)
            .open(&executable)?;
        if io::copy(&mut source.take(max_bytes + 1), &mut output)? > max_bytes {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        output.set_permissions(fs::Permissions::from_mode(0o500))?;
        drop(output);
        File::open(executable)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("connector"));
        let _ = fs::remove_dir(&self.0);
    }
}

/// Refuses interpreter-bearing ELF, scripts, other ABIs, and extended header
/// numbering. The kernel validates the executable itself at exec.
pub(crate) fn check_static_elf(file: &File) -> io::Result<()> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0)?;
    if header[..7] != *b"\x7fELF\x02\x01\x01"
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || header[18..20] != [62, 0]
        || header[54..56] != [56, 0]
    {
        return Err(invalid());
    }
    let count = u16::from_le_bytes([header[56], header[57]]);
    let offset = u64::from_le_bytes(header[32..40].try_into().expect("ELF header width"));
    let end = offset
        .checked_add(u64::from(count) * 56)
        .ok_or_else(invalid)?;
    if offset < 64 || count == 0 || count == u16::MAX || end > file.metadata()?.len() {
        return Err(invalid());
    }
    let mut kind = [0; 4];
    for entry in 0..u64::from(count) {
        file.read_exact_at(&mut kind, offset + 56 * entry)?;
        if u32::from_le_bytes(kind) == 3 {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(crate) fn spawn(mut command: Command, artifact: &File) -> io::Result<Child> {
    let rules = ruleset(artifact)?;
    let mut filter = syscall_filter();
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: the callback uses only syscalls and fixed-size stack values.
    // All descriptors and filter instructions remain live until spawn returns.
    unsafe {
        command.pre_exec(move || {
            checked(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) as libc::c_long)?;
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            for (resource, limit) in [
                (libc::RLIMIT_CORE, 0),
                (libc::RLIMIT_AS, 128 * 1024 * 1024),
                (libc::RLIMIT_CPU, 2),
            ] {
                let limit = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                checked(libc::setrlimit(resource, &limit) as libc::c_long)?;
            }
            checked(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) as libc::c_long)?;
            checked(libc::syscall(
                libc::SYS_landlock_restrict_self,
                rules.as_raw_fd(),
                0,
            ))?;
            checked(libc::syscall(
                libc::SYS_close_range,
                3u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ))?;
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            checked(
                libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program)
                    as libc::c_long,
            )
        });
    }
    command.spawn()
}

fn ruleset(artifact: &File) -> io::Result<OwnedFd> {
    let handled_fs = FS_EXECUTE | FS_READ_FILE | FS_READ_DIR;
    // SAFETY: the ruleset v1 attribute is one u64 and the returned descriptor
    // belongs to this invocation. The packed path rule has the kernel ABI layout.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &handled_fs,
            size_of::<u64>(),
            0,
        )
    };
    checked(fd)?;
    let rules = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
    let path = PathBeneath {
        allowed_access: FS_EXECUTE | FS_READ_FILE,
        parent_fd: artifact.as_raw_fd(),
    };
    checked(unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            rules.as_raw_fd(),
            PATH_BENEATH,
            &path,
            0,
        )
    })?;
    Ok(rules)
}

fn checked(result: libc::c_long) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn syscall_filter() -> Vec<libc::sock_filter> {
    let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
    let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
    let ret = (libc::BPF_RET | libc::BPF_K) as u16;
    let instruction = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
    let mut filter = vec![
        instruction(load, 0, 0, 4), // seccomp_data.arch
        instruction(equal, 1, 0, AUDIT_ARCH_X86_64),
        instruction(ret, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        instruction(load, 0, 0, 0), // seccomp_data.nr
    ];
    for syscall in [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_poll,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_futex,
        libc::SYS_arch_prctl,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_getrandom,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_clock_gettime,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_execve, // Landlock only permits the staged inode.
    ] {
        filter.push(instruction(equal, 0, 1, syscall as u32));
        filter.push(instruction(ret, 0, 0, libc::SECCOMP_RET_ALLOW));
    }
    filter.push(instruction(
        ret,
        0,
        0,
        libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
    ));
    filter
}

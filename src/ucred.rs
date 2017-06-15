use libc::{uid_t, gid_t};

/// Credentials of a process
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct UCred {
    /// UID (user ID) of the process
    pub uid: uid_t,
    /// GID (group ID) of the process
    pub gid: gid_t,
}

#[cfg(target_os = "linux")]
pub use self::impl_linux::get_peer_cred;

#[cfg(target_os = "macos")]
pub use self::impl_macos::get_peer_cred;

#[cfg(target_os = "linux")]
pub mod impl_linux {
    use libc::{getsockopt, SOL_SOCKET, SO_PEERCRED, c_void};
    use std::{io, mem};
    use UnixStream;
    use std::os::unix::io::AsRawFd;

    use libc::ucred as UCred;

    pub fn get_peer_cred(sock: &UnixStream) -> io::Result<super::UCred> {
        unsafe {
            let raw_fd = sock.as_raw_fd();

            let mut ucred: UCred = mem::uninitialized();

            let ucred_size = mem::size_of::<UCred>();

            // These paranoid checks should be optimized-out
            assert!(mem::size_of::<u32>() <= mem::size_of::<usize>());
            assert!(ucred_size <= u32::max_value() as usize);

            let mut ucred_size = ucred_size as u32;
            
            let ret = getsockopt(raw_fd, SOL_SOCKET, SO_PEERCRED, &mut ucred as *mut UCred as *mut c_void, &mut ucred_size);
            if ret == 0 && ucred_size as usize == mem::size_of::<UCred>() {
                Ok(super::UCred {
                    uid: ucred.uid,
                    gid: ucred.gid,
                })
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub mod impl_macos {
    use libc::getpeereid;
    use std::{io, mem};
    use UnixStream;
    use std::os::unix::io::AsRawFd;

    pub fn get_peer_cred(sock: &UnixStream) -> io::Result<super::UCred> {
        unsafe {
            let raw_fd = sock.as_raw_fd();

            let mut cred: super::UCred = mem::uninitialized();

            let ret = getpeereid(raw_fd, &mut cred.uid, &mut cred.pid);

            if ret == 0 {
                Ok(cred)
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

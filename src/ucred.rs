use libc::{gid_t, uid_t};

/// Credentials of a process
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct UCred {
    /// UID (user ID) of the process
    pub uid: uid_t,
    /// GID (group ID) of the process
    pub gid: gid_t,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use self::impl_linux::get_peer_cred;

#[cfg(any(target_os = "dragonfly", target_os = "macos", target_os = "ios", target_os = "freebsd"))]
pub use self::impl_macos::get_peer_cred;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod impl_linux {
    use libc::{c_void, getsockopt, socklen_t, SOL_SOCKET, SO_PEERCRED};
    use std::{io, mem};
    use UnixStream;
    use std::os::unix::io::AsRawFd;

    use libc::ucred;

    pub fn get_peer_cred(sock: &UnixStream) -> io::Result<super::UCred> {
        unsafe {
            let raw_fd = sock.as_raw_fd();

            let mut ucred = ucred {
                pid: 0,
                uid: 0,
                gid: 0,
            };

            let ucred_size = mem::size_of::<ucred>();

            // These paranoid checks should be optimized-out
            assert!(mem::size_of::<u32>() <= mem::size_of::<usize>());
            assert!(ucred_size <= u32::max_value() as usize);

            let mut ucred_size = ucred_size as socklen_t;

            let ret = getsockopt(
                raw_fd,
                SOL_SOCKET,
                SO_PEERCRED,
                &mut ucred as *mut ucred as *mut c_void,
                &mut ucred_size,
            );
            if ret == 0 && ucred_size as usize == mem::size_of::<ucred>() {
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

#[cfg(any(target_os = "dragonfly", target_os = "macos", target_os = "ios", target_os = "freebsd"))]
pub mod impl_macos {
    use libc::getpeereid;
    use std::{io, mem};
    use UnixStream;
    use std::os::unix::io::AsRawFd;

    pub fn get_peer_cred(sock: &UnixStream) -> io::Result<super::UCred> {
        unsafe {
            let raw_fd = sock.as_raw_fd();

            let mut cred: super::UCred = mem::uninitialized();

            let ret = getpeereid(raw_fd, &mut cred.uid, &mut cred.gid);

            if ret == 0 {
                Ok(cred)
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

// Note that SO_PEERCRED is not supported on DragonFly (yet). So do not run tests.
#[cfg(not(target_os = "dragonfly"))]
#[cfg(test)]
mod test {
    use tokio_reactor::Reactor;
    use UnixStream;
    use libc::geteuid;
    use libc::getegid;

    #[test]
    fn test_socket_pair() {
        let core = Reactor::new().unwrap();
        let handle = core.handle();

        let (a, b) = UnixStream::pair(&handle).unwrap();
        let cred_a = a.peer_cred().unwrap();
        let cred_b = b.peer_cred().unwrap();
        assert_eq!(cred_a, cred_b);

        let uid = unsafe { geteuid() };
        let gid = unsafe { getegid() };

        assert_eq!(cred_a.uid, uid);
        assert_eq!(cred_a.gid, gid);
    }
}

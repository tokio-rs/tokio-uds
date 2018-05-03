//! Bindings for Unix Domain Sockets and futures
//!
//! This crate provides bindings between `mio_uds`, the mio crate for Unix
//! Domain sockets, and `futures`. The APIs and bindings in this crate are very
//! similar to the TCP and UDP bindings in the `futures-mio` crate. This crate
//! is also an empty crate on Windows, as Unix Domain Sockets are Unix-specific.

// NB: this is all *very* similar to TCP/UDP, and that's intentional!

#![cfg(unix)]
#![doc(html_root_url = "https://docs.rs/tokio-uds/0.1")]

extern crate bytes;
#[macro_use]
extern crate futures;
extern crate iovec;
extern crate libc;
extern crate log;
extern crate mio;
extern crate mio_uds;
extern crate tokio_io;
extern crate tokio_reactor;

mod incoming;
mod listener;
mod stream;
mod ucred;

pub use incoming::Incoming;
pub use listener::UnixListener;
pub use stream::UnixStream;
pub use ucred::UCred;

use std::fmt;
use std::io;
use std::mem;
use std::net::Shutdown;
use std::os::unix::net::{self, SocketAddr};
use std::os::unix::prelude::*;
use std::path::Path;

use futures::{Async, Future, Poll};

use tokio_reactor::{Handle, PollEvented};
use mio::Ready;



/// An I/O object representing a Unix datagram socket.
pub struct UnixDatagram {
    io: PollEvented<mio_uds::UnixDatagram>,
}

impl UnixDatagram {
    /// Creates a new `UnixDatagram` bound to the specified path.
    pub fn bind<P>(path: P, handle: &Handle) -> io::Result<UnixDatagram>
    where
        P: AsRef<Path>,
    {
        UnixDatagram::_bind(path.as_ref(), handle)
    }

    fn _bind(path: &Path, handle: &Handle) -> io::Result<UnixDatagram> {
        let s = try!(mio_uds::UnixDatagram::bind(path));
        UnixDatagram::new(s, handle)
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// This function will create a pair of interconnected unix sockets for
    /// communicating back and forth between one another. Each socket will be
    /// associated with the event loop whose handle is also provided.
    pub fn pair(handle: &Handle) -> io::Result<(UnixDatagram, UnixDatagram)> {
        let (a, b) = try!(mio_uds::UnixDatagram::pair());
        let a = try!(UnixDatagram::new(a, handle));
        let b = try!(UnixDatagram::new(b, handle));
        Ok((a, b))
    }

    /// Consumes a `UnixDatagram` in the standard library and returns a
    /// nonblocking `UnixDatagram` from this crate.
    ///
    /// The returned datagram will be associated with the given event loop
    /// specified by `handle` and is ready to perform I/O.
    pub fn from_std(datagram: net::UnixDatagram, handle: &Handle) -> io::Result<UnixDatagram> {
        let s = try!(mio_uds::UnixDatagram::from_datagram(datagram));
        UnixDatagram::new(s, handle)
    }

    fn new(socket: mio_uds::UnixDatagram, handle: &Handle) -> io::Result<UnixDatagram> {
        let io = try!(PollEvented::new_with_handle(socket, handle));
        Ok(UnixDatagram { io: io })
    }

    /// Creates a new `UnixDatagram` which is not bound to any address.
    pub fn unbound(handle: &Handle) -> io::Result<UnixDatagram> {
        let s = try!(mio_uds::UnixDatagram::unbound());
        UnixDatagram::new(s, handle)
    }

    /// Connects the socket to the specified address.
    ///
    /// The `send` method may be used to send data to the specified address.
    /// `recv` and `recv_from` will only receive data from that address.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.io.get_ref().connect(path)
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read_ready(&self, ready: Ready) -> Poll<Ready, io::Error> {
        self.io.poll_read_ready(ready)
    }

    /// Test whether this socket is ready to be written to or not.
    pub fn poll_write_ready(&self) -> Poll<Ready, io::Error> {
        self.io.poll_write_ready()
    }

    /// Returns the local address that this socket is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    /// Returns the address of this socket's peer.
    ///
    /// The `connect` method will connect the socket to a peer.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().peer_addr()
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read and the address from
    /// whence the data came.
    pub fn poll_recv_from(&self, buf: &mut [u8]) -> Poll<(usize, SocketAddr), io::Error> {
        if self.io.poll_read_ready(Ready::readable())?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().recv_from(buf);
        if is_wouldblock(&r) {
            self.io.clear_read_ready(Ready::readable())?;
        }
        r.map(Async::Ready)
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read.
    pub fn poll_recv(&self, buf: &mut [u8]) -> Poll<usize, io::Error> {
        if self.io.poll_read_ready(Ready::readable())?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().recv(buf);
        if is_wouldblock(&r) {
            self.io.clear_read_ready(Ready::readable())?;
        }
        r.map(Async::Ready)
    }

    /// Returns a future for receiving a datagram. See the documentation on RecvDgram for details.
    pub fn recv_dgram<T>(self, buf: T) -> RecvDgram<T>
    where
        T: AsMut<[u8]>,
    {
        RecvDgram {
            st: RecvDgramState::Receiving {
                sock: self,
                buf: buf,
            },
        }
    }

    /// Sends data on the socket to the specified address.
    ///
    /// On success, returns the number of bytes written.
    pub fn poll_send_to<P>(&self, buf: &[u8], path: P) -> Poll<usize, io::Error>
    where
        P: AsRef<Path>,
    {
        if self.io.poll_write_ready()?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().send_to(buf, path);
        if is_wouldblock(&r) {
            self.io.clear_write_ready()?;
        }
        r.map(Async::Ready)
    }

    /// Sends data on the socket to the socket's peer.
    ///
    /// The peer address may be set by the `connect` method, and this method
    /// will return an error if the socket has not already been connected.
    ///
    /// On success, returns the number of bytes written.
    pub fn poll_send(&self, buf: &[u8]) -> Poll<usize, io::Error> {
        if self.io.poll_write_ready()?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().send(buf);
        if is_wouldblock(&r) {
            self.io.clear_write_ready()?;
        }
        r.map(Async::Ready)
    }

    /// Returns a future sending the data in buf to the socket at path.
    pub fn send_dgram<T, P>(self, buf: T, path: P) -> SendDgram<T, P>
    where
        T: AsRef<[u8]>,
        P: AsRef<Path>,
    {
        SendDgram {
            st: SendDgramState::Sending {
                sock: self,
                buf: buf,
                addr: path,
            },
        }
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io.get_ref().take_error()
    }

    /// Shut down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the
    /// specified portions to immediately return with an appropriate value
    /// (see the documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.io.get_ref().shutdown(how)
    }
}

impl fmt::Debug for UnixDatagram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.io.get_ref().fmt(f)
    }
}

impl AsRawFd for UnixDatagram {
    fn as_raw_fd(&self) -> RawFd {
        self.io.get_ref().as_raw_fd()
    }
}

fn is_wouldblock<T>(r: &io::Result<T>) -> bool {
    match *r {
        Ok(_) => false,
        Err(ref e) => e.kind() == io::ErrorKind::WouldBlock,
    }
}

/// A future for writing a buffer to a Unix datagram socket.
pub struct SendDgram<T, P> {
    st: SendDgramState<T, P>,
}

enum SendDgramState<T, P> {
    /// current state is Sending
    Sending {
        /// the underlying socket
        sock: UnixDatagram,
        /// the buffer to send
        buf: T,
        /// the destination
        addr: P,
    },
    /// neutral state
    Empty,
}

impl<T, P> Future for SendDgram<T, P>
where
    T: AsRef<[u8]>,
    P: AsRef<Path>,
{
    /// Returns the underlying socket and the buffer that was sent.
    type Item = (UnixDatagram, T);
    /// The error that is returned when sending failed.
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let SendDgramState::Sending {
            ref mut sock,
            ref buf,
            ref addr,
        } = self.st
        {
            let n = try_ready!(sock.poll_send_to(buf.as_ref(), addr));
            if n < buf.as_ref().len() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Couldn't send whole buffer".to_string(),
                ));
            }
        } else {
            panic!()
        }
        if let SendDgramState::Sending { sock, buf, addr: _ } =
            mem::replace(&mut self.st, SendDgramState::Empty)
        {
            Ok(Async::Ready((sock, buf)))
        } else {
            panic!()
        }
    }
}

/// A future for receiving datagrams from a Unix datagram socket.
///
/// An example that uses UDP sockets but is still applicable can be found at
/// https://gist.github.com/dermesser/e331094c2ab28fc7f6ba8a16183fe4d5.
pub struct RecvDgram<T> {
    st: RecvDgramState<T>,
}

/// A future similar to RecvDgram, but without allocating and returning the peer's address.
///
/// This can be used if the peer's address is of no interest, so the allocation overhead can be
/// avoided.
enum RecvDgramState<T> {
    #[allow(dead_code)]
    Receiving {
        sock: UnixDatagram,
        buf: T,
    },
    Empty,
}

impl<T> Future for RecvDgram<T>
where
    T: AsMut<[u8]>,
{
    /// RecvDgram yields a tuple of the underlying socket, the receive buffer, how many bytes were
    /// received, and the address (path) of the peer sending the datagram. If the buffer is too small, the
    /// datagram is truncated.
    type Item = (UnixDatagram, T, usize, String);
    /// This future yields io::Error if an error occurred.
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let received;
        let peer;

        if let RecvDgramState::Receiving {
            ref mut sock,
            ref mut buf,
        } = self.st
        {
            let (n, p) = try_ready!(sock.poll_recv_from(buf.as_mut()));
            received = n;

            peer = p.as_pathname().map_or(String::new(), |p| {
                p.to_str().map_or(String::new(), |s| s.to_string())
            });
        } else {
            panic!()
        }

        if let RecvDgramState::Receiving { sock, buf } =
            mem::replace(&mut self.st, RecvDgramState::Empty)
        {
            Ok(Async::Ready((sock, buf, received, peer)))
        } else {
            panic!()
        }
    }
}

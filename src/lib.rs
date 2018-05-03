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

mod datagram;
mod incoming;
mod listener;
mod send_dgram;
mod stream;
mod ucred;

pub use datagram::UnixDatagram;
pub use incoming::Incoming;
pub use listener::UnixListener;
pub use send_dgram::SendDgram;
pub use stream::UnixStream;
pub use ucred::UCred;

use std::io;
use std::mem;

use futures::{Async, Future, Poll};

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

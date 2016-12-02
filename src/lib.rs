//! Bindings for Unix Domain Sockets and futures
//!
//! This crate provides bindings between `mio_uds`, the mio crate for Unix
//! Domain sockets, and `futures`. The APIs and bindings in this crate are very
//! similar to the TCP and UDP bindings in the `futures-mio` crate. This crate
//! is also an empty crate on Windows, as Unix Domain Sockets are Unix-specific.

// NB: this is all *very* similar to TCP/UDP, and that's intentional!

#![cfg(unix)]
#![deny(missing_docs)]

#[macro_use]
extern crate futures;
#[macro_use]
extern crate tokio_core;
extern crate mio;
extern crate mio_uds;
#[macro_use]
extern crate log;

use std::fmt;
use std::io::{self, Read, Write};
use std::mem;
use std::net::Shutdown;
use std::os::unix::net::SocketAddr;
use std::os::unix::prelude::*;
use std::path::Path;

use futures::{Future, Poll, Async};
use futures::stream::Stream;
use tokio_core::reactor::{PollEvented, Handle};
use tokio_core::io::{Io, IoStream};

/// A Unix socket which can accept connections from other unix sockets.
pub struct UnixListener {
    io: PollEvented<mio_uds::UnixListener>,
}

impl UnixListener {
    /// Creates a new `UnixListener` bound to the specified path.
    pub fn bind<P>(path: P, handle: &Handle) -> io::Result<UnixListener>
        where P: AsRef<Path>
    {
        UnixListener::_bind(path.as_ref(), handle)
    }

    fn _bind(path: &Path, handle: &Handle) -> io::Result<UnixListener> {
        let s = try!(mio_uds::UnixListener::bind(path));
        UnixListener::new(s, handle)
    }

    fn new(listener: mio_uds::UnixListener, handle: &Handle) -> io::Result<UnixListener> {
        let io = try!(PollEvented::new(listener, handle));
        Ok(UnixListener { io: io })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read(&self) -> Async<()> {
        self.io.poll_read()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io.get_ref().take_error()
    }

    /// Consumes this listener, returning a stream of the sockets this listener
    /// accepts.
    ///
    /// This method returns an implementation of the `Stream` trait which
    /// resolves to the sockets the are accepted on this listener.
    pub fn incoming(self) -> IoStream<(UnixStream, SocketAddr)> {
        struct Incoming {
            inner: UnixListener,
        }

        impl Stream for Incoming {
            type Item = (mio_uds::UnixStream, SocketAddr);
            type Error = io::Error;

            fn poll(&mut self) -> Poll<Option<Self::Item>, io::Error> {
                if self.inner.io.poll_read().is_not_ready() {
                    return Ok(Async::NotReady);
                }
                match try!(self.inner.io.get_ref().accept()) {
                    Some(pair) => Ok(Some(pair).into()),
                    None => {
                        self.inner.io.need_read();
                        Ok(Async::NotReady)
                    }
                }
            }
        }

        let remote = self.io.remote().clone();
        Incoming { inner: self }
            .and_then(move |(client, addr)| {
                let (tx, rx) = futures::oneshot();
                remote.spawn(move |handle| {
                    let res = PollEvented::new(client, handle)
                        .map(move |io| (UnixStream { io: io }, addr));
                    tx.complete(res);
                    Ok(())
                });
                rx.then(|res| res.expect("shouldn't be canceled"))
            })
            .boxed()
    }
}

impl fmt::Debug for UnixListener {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.io.get_ref().fmt(f)
    }
}

impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.io.get_ref().as_raw_fd()
    }
}

/// A structure representing a connected unix socket.
///
/// This socket can be connected directly with `UnixStream::connect` or accepted
/// from a listener with `UnixListener::incoming`. Additionally, a pair of
/// anonymous Unix sockets can be created with `UnixStream::pair`.
pub struct UnixStream {
    io: PollEvented<mio_uds::UnixStream>,
}

impl UnixStream {
    /// Connects to the socket named by `path`.
    ///
    /// This function will create a new unix socket and connect to the path
    /// specified, performing associating the returned stream with the provided
    /// event loop's handle.
    pub fn connect<P>(p: P, handle: &Handle) -> io::Result<UnixStream>
        where P: AsRef<Path>
    {
        UnixStream::_connect(p.as_ref(), handle)
    }

    fn _connect(path: &Path, handle: &Handle) -> io::Result<UnixStream> {
        let s = try!(mio_uds::UnixStream::connect(path));
        UnixStream::new(s, handle)
    }

    /// Creates an unnamed pair of connected sockets.
    ///
    /// This function will create a pair of interconnected unix sockets for
    /// communicating back and forth between one another. Each socket will be
    /// associated with the event loop whose handle is also provided.
    pub fn pair(handle: &Handle) -> io::Result<(UnixStream, UnixStream)> {
        let (a, b) = try!(mio_uds::UnixStream::pair());
        let a = try!(UnixStream::new(a, handle));
        let b = try!(UnixStream::new(b, handle));
        Ok((a, b))
    }

    fn new(stream: mio_uds::UnixStream, handle: &Handle) -> io::Result<UnixStream> {
        let io = try!(PollEvented::new(stream, handle));
        Ok(UnixStream { io: io })
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read(&self) -> Async<()> {
        self.io.poll_read()
    }

    /// Test whether this socket is writey to be written to or not.
    pub fn poll_write(&self) -> Async<()> {
        self.io.poll_write()
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    /// Returns the socket address of the remote half of this connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().peer_addr()
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io.get_ref().take_error()
    }

    /// Shuts down the read, write, or both halves of this connection.
    ///
    /// This function will cause all pending and future I/O calls on the
    /// specified portions to immediately return with an appropriate value
    /// (see the documentation of `Shutdown`).
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.io.get_ref().shutdown(how)
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.read(buf)
    }
}

impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.io.flush()
    }
}

impl Io for UnixStream {
    fn poll_read(&mut self) -> Async<()> {
        <UnixStream>::poll_read(self)
    }

    fn poll_write(&mut self) -> Async<()> {
        <UnixStream>::poll_write(self)
    }
}

impl<'a> Read for &'a UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.io).read(buf)
    }
}

impl<'a> Write for &'a UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.io).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.io).flush()
    }
}

impl<'a> Io for &'a UnixStream {
    fn poll_read(&mut self) -> Async<()> {
        <UnixStream>::poll_read(self)
    }

    fn poll_write(&mut self) -> Async<()> {
        <UnixStream>::poll_write(self)
    }
}

impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.io.get_ref().fmt(f)
    }
}

impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.io.get_ref().as_raw_fd()
    }
}

/// An I/O object representing a Unix datagram socket.
pub struct UnixDatagram {
    io: PollEvented<mio_uds::UnixDatagram>,
}

impl UnixDatagram {
    /// Creates a new `UnixDatagram` bound to the specified path.
    pub fn bind<P>(path: P, handle: &Handle) -> io::Result<UnixDatagram>
        where P: AsRef<Path>
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


    fn new(socket: mio_uds::UnixDatagram, handle: &Handle) -> io::Result<UnixDatagram> {
        let io = try!(PollEvented::new(socket, handle));
        Ok(UnixDatagram { io: io })
    }

    /// Connects the socket to the specified address.
    ///
    /// The `send` method may be used to send data to the specified address.
    /// `recv` and `recv_from` will only receive data from that address.
    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.io.get_ref().connect(path)
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read(&self) -> Async<()> {
        self.io.poll_read()
    }

    /// Test whether this socket is writey to be written to or not.
    pub fn poll_write(&self) -> Async<()> {
        self.io.poll_write()
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
    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        if self.io.poll_read().is_not_ready() {
            return Err(mio::would_block());
        }
        let r = self.io.get_ref().recv_from(buf);
        if is_wouldblock(&r) {
            self.io.need_read();
        }
        return r;
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        if self.io.poll_read().is_not_ready() {
            return Err(mio::would_block());
        }
        let r = self.io.get_ref().recv(buf);
        if is_wouldblock(&r) {
            self.io.need_read();
        }
        return r;
    }

    /// Returns a future for receiving a datagram. See the documentation on RecvDgram for details.
    pub fn recv_dgram<T>(self, buf: T) -> RecvDgram<T>
        where T: AsMut<[u8]>
    {
        RecvDgram {
            st: RecvDgramState::Receiving {
                sock: self,
                buf: buf,
            },
        }
    }

    /// Returns a future for receiving a datagram without the sender's address. This avoids
    /// allocation overhead.
    pub fn recv_anon_dgram<T>(self, buf: T) -> RecvAnonDgram<T>
        where T: AsMut<[u8]>
    {
        RecvAnonDgram {
            st: RecvDgramState::Receiving {
                sock: self,
                buf: buf,
            },
        }
    }

    /// Sends data on the socket to the specified address.
    ///
    /// On success, returns the number of bytes written.
    pub fn send_to<P>(&self, buf: &[u8], path: P) -> io::Result<usize>
        where P: AsRef<Path>
    {
        if self.io.poll_write().is_not_ready() {
            return Err(mio::would_block());
        }
        let r = self.io.get_ref().send_to(buf, path);
        if is_wouldblock(&r) {
            self.io.need_write();
        }
        return r;
    }

    /// Sends data on the socket to the socket's peer.
    ///
    /// The peer address may be set by the `connect` method, and this method
    /// will return an error if the socket has not already been connected.
    ///
    /// On success, returns the number of bytes written.
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        if self.io.poll_write().is_not_ready() {
            return Err(mio::would_block());
        }
        let r = self.io.get_ref().send(buf);
        if is_wouldblock(&r) {
            self.io.need_write();
        }
        return r;
    }


    /// Returns a future sending the data in buf to the socket at path.
    pub fn send_dgram<T, P>(self, buf: T, path: P) -> SendDgram<T, P>
        where T: AsRef<[u8]>,
              P: AsRef<Path>
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
    where T: AsRef<[u8]>,
          P: AsRef<Path>
{
    /// Returns the underlying socket and the buffer that was sent.
    type Item = (UnixDatagram, T);
    /// The error that is returned when sending failed.
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let SendDgramState::Sending { ref sock, ref buf, ref addr } = self.st {
            let n = try_nb!(sock.send_to(buf.as_ref(), addr));
            if n < buf.as_ref().len() {
                return Err(io::Error::new(io::ErrorKind::Other,
                                          "Couldn't send whole buffer".to_string()));
            }
        } else {
            panic!()
        }
        if let SendDgramState::Sending { sock, buf, addr: _ } =
               mem::replace(&mut self.st, SendDgramState::Empty) {
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
pub struct RecvAnonDgram<T> {
    st: RecvDgramState<T>,
}

enum RecvDgramState<T> {
    #[allow(dead_code)]
    Receiving { sock: UnixDatagram, buf: T },
    Empty,
}

impl<T> Future for RecvDgram<T>
    where T: AsMut<[u8]>
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

        if let RecvDgramState::Receiving { ref sock, ref mut buf } = self.st {
            let (n, p) = try_nb!(sock.recv_from(buf.as_mut()));
            received = n;

            peer = p.as_pathname().map_or(String::new(),
                                          |p| p.to_str().map_or(String::new(), |s| s.to_string()));

        } else {
            panic!()
        }

        if let RecvDgramState::Receiving { sock, buf } = mem::replace(&mut self.st,
                                                                      RecvDgramState::Empty) {
            Ok(Async::Ready((sock, buf, received, peer)))
        } else {
            panic!()
        }
    }
}

impl<T> Future for RecvAnonDgram<T>
    where T: AsMut<[u8]>
{
    /// RecvDgram yields a tuple of the underlying socket, the receive buffer, how many bytes were
    /// received, and the address (path) of the peer sending the datagram. If the buffer is too small, the
    /// datagram is truncated.
    type Item = (UnixDatagram, T, usize);
    /// This future yields io::Error if an error occurred.
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let received;

        if let RecvDgramState::Receiving { ref sock, ref mut buf } = self.st {
            let n = try_nb!(sock.recv(buf.as_mut()));
            received = n;
        } else {
            panic!()
        }

        if let RecvDgramState::Receiving { sock, buf } = mem::replace(&mut self.st,
                                                                      RecvDgramState::Empty) {
            Ok(Async::Ready((sock, buf, received)))
        } else {
            panic!()
        }
    }
}

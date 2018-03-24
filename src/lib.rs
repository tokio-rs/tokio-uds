//! Bindings for Unix Domain Sockets and futures
//!
//! This crate provides bindings between `mio_uds`, the mio crate for Unix
//! Domain sockets, and `futures`. The APIs and bindings in this crate are very
//! similar to the TCP and UDP bindings in the `futures-mio` crate. This crate
//! is also an empty crate on Windows, as Unix Domain Sockets are Unix-specific.

// NB: this is all *very* similar to TCP/UDP, and that's intentional!

#![cfg(unix)]
#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/tokio-uds/0.1")]

extern crate bytes;
#[macro_use]
extern crate futures;
#[cfg(feature = "unstable-futures")]
extern crate futures2;
extern crate iovec;
extern crate libc;
#[macro_use]
extern crate log;
extern crate mio;
extern crate mio_uds;
#[macro_use]
extern crate tokio_io;
extern crate tokio_reactor;

use std::fmt;
use std::io::{self, Read, Write};
use std::mem;
use std::net::Shutdown;
use std::os::unix::net::{self, SocketAddr};
use std::os::unix::prelude::*;
use std::path::Path;

use bytes::{Buf, BufMut};
use futures::{Async, Future, Poll, Stream};
#[cfg(feature = "unstable-futures")]
use futures2::task;
use tokio_io::{AsyncRead, AsyncWrite};
use iovec::IoVec;
use tokio_reactor::{Handle, PollEvented};
use mio::Ready;

#[cfg(feature = "unstable-futures")]
macro_rules! try_ready2 {
    ($e: expr) => {
        match $e {
            Ok($crate::futures2::Async::Ready(t)) => t,
            Ok($crate::futures2::Async::Pending) => return Ok($crate::futures2::Async::Pending),
            Err(e) => return Err(From::from(e)),
        }
    };
}

mod frame;
pub use frame::{UnixDatagramCodec, UnixDatagramFramed};
mod ucred;
pub use ucred::UCred;

#[cfg(feature = "unstable-futures")]
fn lift_async<T>(old: futures::Async<T>) -> futures2::Async<T> {
    match old {
        futures::Async::Ready(x) => futures2::Async::Ready(x),
        futures::Async::NotReady => futures2::Async::Pending,
    }
}

/// Stream of listeners
pub struct Incoming {
    inner: UnixListener,
}

impl Stream for Incoming {
    type Item = (UnixStream, SocketAddr);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, io::Error> {
        Ok(Some(try_nb!(self.inner.accept())).into())
    }
}

#[cfg(feature = "unstable-futures")]
impl futures2::Stream for Incoming {
    type Item = (UnixStream, SocketAddr);
    type Error = io::Error;

    fn poll_next(
        &mut self,
        cx: &mut task::Context,
    ) -> futures2::Poll<Option<Self::Item>, io::Error> {
        Ok(Some(try_ready2!(self.inner.poll_accept2(cx))).into())
    }
}

/// A Unix socket which can accept connections from other unix sockets.
pub struct UnixListener {
    io: PollEvented<mio_uds::UnixListener>,
    handle: Handle,
}

impl UnixListener {
    /// Creates a new `UnixListener` bound to the specified path.
    pub fn bind<P>(path: P, handle: Handle) -> io::Result<UnixListener>
    where
        P: AsRef<Path>,
    {
        UnixListener::_bind(path.as_ref(), handle)
    }

    /// Consumes a `UnixListener` in the standard library and returns a
    /// nonblocking `UnixListener` from this crate.
    ///
    /// The returned listener will be associated with the given event loop
    /// specified by `handle` and is ready to perform I/O.
    pub fn from_listener(listener: net::UnixListener, handle: Handle) -> io::Result<UnixListener> {
        let s = try!(mio_uds::UnixListener::from_listener(listener));
        UnixListener::new(s, handle)
    }

    fn _bind(path: &Path, handle: Handle) -> io::Result<UnixListener> {
        let s = try!(mio_uds::UnixListener::bind(path));
        UnixListener::new(s, handle)
    }

    fn new(listener: mio_uds::UnixListener, handle: Handle) -> io::Result<UnixListener> {
        let io = try!(PollEvented::new_with_handle(listener, &handle));
        Ok(UnixListener {
            io: io,
            handle: handle,
        })
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    /// Test whether this socket is ready to be read or not.
    #[cfg(feature = "unstable-futures")]
    pub fn poll_read_ready2(
        &self,
        cx: &mut task::Context,
        ready: Ready,
    ) -> futures2::Poll<Ready, io::Error> {
        self.io.poll_read_ready2(cx, ready)
    }

    /// Returns the value of the `SO_ERROR` option.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.io.get_ref().take_error()
    }

    /// Attempt to accept a connection and create a new connected `UnixStream`
    /// if successful.
    ///
    /// This function will attempt an accept operation, but will not block
    /// waiting for it to complete. If the operation would block then a "would
    /// block" error is returned. Additionally, if this method would block, it
    /// registers the current task to receive a notification when it would
    /// otherwise not block.
    ///
    /// Note that typically for simple usage it's easier to treat incoming
    /// connections as a `Stream` of `UnixStream`s with the `incoming` method
    /// below.
    ///
    /// # Panics
    ///
    /// This function will panic if it is called outside the context of a
    /// future's task. It's recommended to only call this from the
    /// implementation of a `Future::poll`, if necessary.
    pub fn accept(&mut self) -> io::Result<(UnixStream, SocketAddr)> {
        loop {
            if let Async::NotReady = self.io.poll_read_ready(Ready::readable())? {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "not ready"));
            }

            match try!(self.io.get_ref().accept()) {
                None => {
                    self.io.clear_read_ready(Ready::readable())?;
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, "not ready"));
                }
                Some((sock, addr)) => {
                    let io = try!(PollEvented::new_with_handle(sock, &self.handle));
                    return Ok((UnixStream { io: io }, addr));
                }
            }
        }
    }

    /// Attempt to accept a connection and create a new connected `UnixStream`
    /// if successful.
    ///
    /// This function will attempt an accept operation, but will not block
    /// waiting for it to complete. If the operation would block then a "would
    /// block" error is returned. Additionally, if this method would block, it
    /// registers the current task to receive a notification when it would
    /// otherwise not block.
    ///
    /// Note that typically for simple usage it's easier to treat incoming
    /// connections as a `Stream` of `UnixStream`s with the `incoming` method
    /// below.
    ///
    /// # Panics
    ///
    /// This function will panic if it is called outside the context of a
    /// future's task. It's recommended to only call this from the
    /// implementation of a `Future::poll`, if necessary.
    #[cfg(feature = "unstable-futures")]
    pub fn poll_accept2(
        &mut self,
        cx: &mut task::Context,
    ) -> futures2::Poll<(UnixStream, SocketAddr), io::Error> {
        use futures2::Async;
        try_ready2!(self.io.poll_read_ready2(cx, Ready::readable()));

        match try!(self.io.get_ref().accept()) {
            None => {
                self.io.clear_read_ready(Ready::readable())?;
                Ok(Async::Pending)
            }
            Some((sock, addr)) => {
                let io = try!(PollEvented::new_with_handle(sock, &self.handle));
                Ok(Async::Ready((UnixStream { io: io }, addr)))
            }
        }
    }

    /// Consumes this listener, returning a stream of the sockets this listener
    /// accepts.
    ///
    /// This method returns an implementation of the `Stream` trait which
    /// resolves to the sockets the are accepted on this listener.
    pub fn incoming(self) -> Incoming {
        Incoming { inner: self }
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
    /// specified, associating the returned stream with the default event loop's
    /// handle.
    pub fn connect<P>(p: P) -> io::Result<UnixStream>
    where
        P: AsRef<Path>,
    {
        UnixStream::connect_handle(p.as_ref(), &Handle::default())
    }

    /// Connects to the socket named by `path`.
    ///
    /// This function will create a new unix socket and connect to the path
    /// specified, associating the returned stream with the provided
    /// event loop's handle.
    pub fn connect_handle<P>(p: P, handle: &Handle) -> io::Result<UnixStream>
    where
        P: AsRef<Path>,
    {
        UnixStream::_connect(p.as_ref(), handle)
    }

    fn _connect(path: &Path, handle: &Handle) -> io::Result<UnixStream> {
        let s = try!(mio_uds::UnixStream::connect(path));
        UnixStream::new(s, handle)
    }

    /// Consumes a `UnixStream` in the standard library and returns a
    /// nonblocking `UnixStream` from this crate.
    ///
    /// The returned stream will be associated with the given event loop
    /// specified by `handle` and is ready to perform I/O.
    pub fn from_stream(stream: net::UnixStream, handle: &Handle) -> io::Result<UnixStream> {
        let s = try!(mio_uds::UnixStream::from_stream(stream));
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
        let io = try!(PollEvented::new_with_handle(stream, handle));
        Ok(UnixStream { io: io })
    }

    /// Test whether this socket is ready to be read or not.
    pub fn poll_read_ready(&self, ready: Ready) -> Poll<Ready, io::Error> {
        self.io.poll_read_ready(ready)
    }

    /// Test whether this socket is ready to be written to or not.
    pub fn poll_write_ready(&self) -> Poll<Ready, io::Error> {
        self.io.poll_write_ready()
    }

    /// Test whether this socket is ready to be read or not.
    #[cfg(feature = "unstable-futures")]
    pub fn poll_read_ready2(
        &self,
        cx: &mut task::Context,
        ready: Ready,
    ) -> futures2::Poll<Ready, io::Error> {
        self.io.poll_read_ready2(cx, ready)
    }

    /// Test whether this socket is ready to be written to or not.
    #[cfg(feature = "unstable-futures")]
    pub fn poll_write_ready2(&self, cx: &mut task::Context) -> futures2::Poll<Ready, io::Error> {
        self.io.poll_write_ready2(cx)
    }

    /// Returns the socket address of the local half of this connection.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().local_addr()
    }

    /// Returns the socket address of the remote half of this connection.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.io.get_ref().peer_addr()
    }

    /// Returns effective credentials of the process which called `connect` or `socketpair`.
    pub fn peer_cred(&self) -> io::Result<UCred> {
        ucred::get_peer_cred(self)
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

impl AsyncRead for UnixStream {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }

    fn read_buf<B: BufMut>(&mut self, buf: &mut B) -> Poll<usize, io::Error> {
        <&UnixStream>::read_buf(&mut &*self, buf)
    }
}

impl AsyncWrite for UnixStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        <&UnixStream>::shutdown(&mut &*self)
    }

    fn write_buf<B: Buf>(&mut self, buf: &mut B) -> Poll<usize, io::Error> {
        <&UnixStream>::write_buf(&mut &*self, buf)
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

#[cfg(feature = "unstable-futures")]
impl futures2::io::AsyncRead for UnixStream {
    fn poll_read(
        &mut self,
        cx: &mut task::Context,
        buf: &mut [u8],
    ) -> futures2::Poll<usize, io::Error> {
        futures2::io::AsyncRead::poll_read(&mut &*self, cx, buf)
    }

    fn poll_vectored_read(
        &mut self,
        cx: &mut task::Context,
        vec: &mut [&mut IoVec],
    ) -> futures2::Poll<usize, io::Error> {
        futures2::io::AsyncRead::poll_vectored_read(&mut &*self, cx, vec)
    }

    unsafe fn initializer(&self) -> futures2::io::Initializer {
        futures2::io::AsyncRead::initializer(&self.io)
    }
}

#[cfg(feature = "unstable-futures")]
impl<'a> futures2::io::AsyncRead for &'a UnixStream {
    fn poll_read(
        &mut self,
        cx: &mut task::Context,
        buf: &mut [u8],
    ) -> futures2::Poll<usize, io::Error> {
        (&self.io).poll_read(cx, buf)
    }

    fn poll_vectored_read(
        &mut self,
        cx: &mut task::Context,
        vec: &mut [&mut IoVec],
    ) -> futures2::Poll<usize, io::Error> {
        use futures2::Async;
        try_ready2!(<UnixStream>::poll_read_ready2(self, cx, Ready::readable()));
        unsafe {
            let r = read_ready_vecs(vec, self.as_raw_fd());
            if r == -1 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    self.io.clear_write_ready()?;
                    Ok(Async::Pending)
                } else {
                    Err(e)
                }
            } else {
                let r = r as usize;
                Ok(r.into())
            }
        }
    }

    unsafe fn initializer(&self) -> futures2::io::Initializer {
        futures2::io::AsyncRead::initializer(&self.io)
    }
}

#[cfg(feature = "unstable-futures")]
impl futures2::io::AsyncWrite for UnixStream {
    fn poll_write(
        &mut self,
        cx: &mut task::Context,
        buf: &[u8],
    ) -> futures2::Poll<usize, io::Error> {
        (&self.io).poll_write(cx, buf)
    }

    fn poll_vectored_write(
        &mut self,
        cx: &mut task::Context,
        vec: &[&IoVec],
    ) -> futures2::Poll<usize, io::Error> {
        (&*self).poll_vectored_write(cx, vec)
    }

    fn poll_flush(&mut self, cx: &mut task::Context) -> futures2::Poll<(), io::Error> {
        (&self.io).poll_flush(cx)
    }

    fn poll_close(&mut self, cx: &mut task::Context) -> futures2::Poll<(), io::Error> {
        (&self.io).poll_close(cx)
    }
}

#[cfg(feature = "unstable-futures")]
impl<'a> futures2::io::AsyncWrite for &'a UnixStream {
    fn poll_write(
        &mut self,
        cx: &mut task::Context,
        buf: &[u8],
    ) -> futures2::Poll<usize, io::Error> {
        (&self.io).poll_write(cx, buf)
    }

    fn poll_vectored_write(
        &mut self,
        cx: &mut task::Context,
        vec: &[&IoVec],
    ) -> futures2::Poll<usize, io::Error> {
        use futures2::Async;
        try_ready2!(<UnixStream>::poll_write_ready2(self, cx));
        unsafe {
            let r = write_ready_vecs(vec, self.as_raw_fd());
            if r == -1 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    self.io.clear_write_ready()?;
                    Ok(Async::Pending)
                } else {
                    Err(e)
                }
            } else {
                let r = r as usize;
                Ok(r.into())
            }
        }
    }

    fn poll_flush(&mut self, cx: &mut task::Context) -> futures2::Poll<(), io::Error> {
        (&self.io).poll_flush(cx)
    }

    fn poll_close(&mut self, cx: &mut task::Context) -> futures2::Poll<(), io::Error> {
        (&self.io).poll_close(cx)
    }
}

unsafe fn read_ready<B: BufMut>(buf: &mut B, raw_fd: RawFd) -> isize {
    // The `IoVec` type can't have a 0-length size, so we create a bunch
    // of dummy versions on the stack with 1 length which we'll quickly
    // overwrite.
    let b1: &mut [u8] = &mut [0];
    let b2: &mut [u8] = &mut [0];
    let b3: &mut [u8] = &mut [0];
    let b4: &mut [u8] = &mut [0];
    let b5: &mut [u8] = &mut [0];
    let b6: &mut [u8] = &mut [0];
    let b7: &mut [u8] = &mut [0];
    let b8: &mut [u8] = &mut [0];
    let b9: &mut [u8] = &mut [0];
    let b10: &mut [u8] = &mut [0];
    let b11: &mut [u8] = &mut [0];
    let b12: &mut [u8] = &mut [0];
    let b13: &mut [u8] = &mut [0];
    let b14: &mut [u8] = &mut [0];
    let b15: &mut [u8] = &mut [0];
    let b16: &mut [u8] = &mut [0];
    let mut bufs: [&mut IoVec; 16] = [
        b1.into(),
        b2.into(),
        b3.into(),
        b4.into(),
        b5.into(),
        b6.into(),
        b7.into(),
        b8.into(),
        b9.into(),
        b10.into(),
        b11.into(),
        b12.into(),
        b13.into(),
        b14.into(),
        b15.into(),
        b16.into(),
    ];

    let n = buf.bytes_vec_mut(&mut bufs);
    read_ready_vecs(&mut bufs[..n], raw_fd)
}

unsafe fn read_ready_vecs(bufs: &mut [&mut IoVec], raw_fd: RawFd) -> isize {
    let iovecs = iovec::unix::as_os_slice_mut(bufs);

    libc::readv(raw_fd, iovecs.as_ptr(), iovecs.len() as i32)
}

impl<'a> AsyncRead for &'a UnixStream {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }

    fn read_buf<B: BufMut>(&mut self, buf: &mut B) -> Poll<usize, io::Error> {
        if let Async::NotReady = <UnixStream>::poll_read_ready(self, Ready::readable())? {
            return Ok(Async::NotReady);
        }
        unsafe {
            let r = read_ready(buf, self.as_raw_fd());
            if r == -1 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    self.io.clear_write_ready()?;
                    Ok(Async::NotReady)
                } else {
                    Err(e)
                }
            } else {
                let r = r as usize;
                buf.advance_mut(r);
                Ok(r.into())
            }
        }
    }
}

unsafe fn write_ready<B: Buf>(buf: &mut B, raw_fd: RawFd) -> isize {
    // The `IoVec` type can't have a zero-length size, so create a dummy
    // version from a 1-length slice which we'll overwrite with the
    // `bytes_vec` method.
    static DUMMY: &[u8] = &[0];
    let iovec = <&IoVec>::from(DUMMY);
    let mut bufs = [
        iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec, iovec,
        iovec, iovec, iovec,
    ];

    let n = buf.bytes_vec(&mut bufs);
    write_ready_vecs(&bufs[..n], raw_fd)
}

unsafe fn write_ready_vecs(bufs: &[&IoVec], raw_fd: RawFd) -> isize {
    let iovecs = iovec::unix::as_os_slice(bufs);

    libc::writev(raw_fd, iovecs.as_ptr(), iovecs.len() as i32)
}

impl<'a> AsyncWrite for &'a UnixStream {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        Ok(().into())
    }

    fn write_buf<B: Buf>(&mut self, buf: &mut B) -> Poll<usize, io::Error> {
        if let Async::NotReady = <UnixStream>::poll_write_ready(self)? {
            return Ok(Async::NotReady);
        }
        unsafe {
            let r = write_ready(buf, self.as_raw_fd());
            if r == -1 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    self.io.clear_write_ready()?;
                    Ok(Async::NotReady)
                } else {
                    Err(e)
                }
            } else {
                let r = r as usize;
                buf.advance(r);
                Ok(r.into())
            }
        }
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
    pub fn from_datagram(datagram: net::UnixDatagram, handle: &Handle) -> io::Result<UnixDatagram> {
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
    pub fn recv_from(&mut self, buf: &mut [u8]) -> Poll<(usize, SocketAddr), io::Error> {
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
    pub fn recv(&mut self, buf: &mut [u8]) -> Poll<usize, io::Error> {
        if self.io.poll_read_ready(Ready::readable())?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().recv(buf);
        if is_wouldblock(&r) {
            self.io.clear_read_ready(Ready::readable())?;
        }
        r.map(Async::Ready)
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read and the address from
    /// whence the data came.
    #[cfg(feature = "unstable-futures")]
    pub fn recv_from2(
        &mut self,
        cx: &mut task::Context,
        buf: &mut [u8],
    ) -> futures2::Poll<(usize, SocketAddr), io::Error> {
        use futures2::Async;
        try_ready2!(self.io.poll_read_ready2(cx, Ready::readable()));
        let r = self.io.get_ref().recv_from(buf);
        if is_wouldblock(&r) {
            self.io.clear_read_ready2(cx, Ready::readable())?;
        }
        r.map(Async::Ready)
    }

    /// Receives data from the socket.
    ///
    /// On success, returns the number of bytes read.
    #[cfg(feature = "unstable-futures")]
    pub fn recv2(
        &mut self,
        cx: &mut task::Context,
        buf: &mut [u8],
    ) -> futures2::Poll<usize, io::Error> {
        use futures2::Async;
        try_ready2!(self.io.poll_read_ready2(cx, Ready::readable()));
        let r = self.io.get_ref().recv(buf);
        if is_wouldblock(&r) {
            self.io.clear_read_ready2(cx, Ready::readable())?;
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
    pub fn send_to<P>(&mut self, buf: &[u8], path: P) -> Poll<usize, io::Error>
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
    pub fn send(&mut self, buf: &[u8]) -> Poll<usize, io::Error> {
        if self.io.poll_write_ready()?.is_not_ready() {
            return Ok(Async::NotReady);
        }
        let r = self.io.get_ref().send(buf);
        if is_wouldblock(&r) {
            self.io.clear_write_ready()?;
        }
        r.map(Async::Ready)
    }

    /// Sends data on the socket to the specified address.
    ///
    /// On success, returns the number of bytes written.
    #[cfg(feature = "unstable-futures")]
    pub fn send_to2<P>(
        &mut self,
        cx: &mut task::Context,
        buf: &[u8],
        path: P,
    ) -> futures2::Poll<usize, io::Error>
    where
        P: AsRef<Path>,
    {
        use futures2::Async;
        try_ready2!(self.io.poll_write_ready2(cx));
        let r = self.io.get_ref().send_to(buf, path);
        if is_wouldblock(&r) {
            self.io.clear_write_ready2(cx)?;
        }
        r.map(Async::Ready)
    }

    /// Sends data on the socket to the socket's peer.
    ///
    /// The peer address may be set by the `connect` method, and this method
    /// will return an error if the socket has not already been connected.
    ///
    /// On success, returns the number of bytes written.
    #[cfg(feature = "unstable-futures")]
    pub fn send2(
        &mut self,
        cx: &mut task::Context,
        buf: &[u8],
    ) -> futures2::Poll<usize, io::Error> {
        use futures2::Async;
        try_ready2!(self.io.poll_write_ready2(cx));
        let r = self.io.get_ref().send(buf);
        if is_wouldblock(&r) {
            self.io.clear_write_ready2(cx)?;
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

    /// Provides a `Stream` and `Sink` interface for reading and writing to
    /// this `UnixDatagram` object, using the provided `UnixDatagramCodec` to
    /// read and write the raw data.
    ///
    /// Raw `UnixDatagram` sockets work with datagrams, but higher-level code
    /// usually wants to batch these into meaningful chunks, called "frames".
    /// This method layers framing on top of this socket by using the
    /// `UnixDatagramCodec` trait to handle encoding and decoding of messages
    /// frames. Note that the incoming and outgoing frame types may be distinct.
    ///
    /// This function returns a *single* object that is both `Stream` and
    /// `Sink`; grouping this into a single object is often useful for layering
    /// things which require both read and write access to the underlying
    /// object.
    ///
    /// If you want to work more directly with the streams and sink, consider
    /// calling `split` on the `UnixDatagramFramed` returned by this method,
    /// which will break them into separate objects, allowing them to interact
    /// more easily.
    pub fn framed<C>(self, codec: C) -> UnixDatagramFramed<C>
    where
        C: UnixDatagramCodec,
    {
        frame::new(self, codec)
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
            let n = try_ready!(sock.send_to(buf.as_ref(), addr));
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

#[cfg(feature = "unstable-futures")]
impl<T, P> futures2::Future for SendDgram<T, P>
where
    T: AsRef<[u8]>,
    P: AsRef<Path>,
{
    /// Returns the underlying socket and the buffer that was sent.
    type Item = (UnixDatagram, T);
    /// The error that is returned when sending failed.
    type Error = io::Error;

    fn poll(&mut self, cx: &mut task::Context) -> futures2::Poll<Self::Item, Self::Error> {
        use futures2::Async;
        if let SendDgramState::Sending {
            ref mut sock,
            ref buf,
            ref addr,
        } = self.st
        {
            let n = try_ready2!(sock.send_to2(cx, buf.as_ref(), addr));
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
            let (n, p) = try_ready!(sock.recv_from(buf.as_mut()));
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

#[cfg(feature = "unstable-futures")]
impl<T> futures2::Future for RecvDgram<T>
where
    T: AsMut<[u8]>,
{
    /// RecvDgram yields a tuple of the underlying socket, the receive buffer, how many bytes were
    /// received, and the address (path) of the peer sending the datagram. If the buffer is too small, the
    /// datagram is truncated.
    type Item = (UnixDatagram, T, usize, String);
    /// This future yields io::Error if an error occurred.
    type Error = io::Error;

    fn poll(&mut self, _cx: &mut task::Context) -> futures2::Poll<Self::Item, Self::Error> {
        use futures2::Async;

        let received;
        let peer;

        if let RecvDgramState::Receiving {
            ref mut sock,
            ref mut buf,
        } = self.st
        {
            let (n, p) = try_ready2!(sock.recv_from(buf.as_mut()).map(::lift_async));
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

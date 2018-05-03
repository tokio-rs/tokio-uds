//! Bindings for Unix Domain Sockets and futures
//!
//! This crate provides bindings between `mio_uds`, the mio crate for Unix
//! Domain sockets, and `futures`. The APIs and bindings in this crate are very
//! similar to the TCP and UDP bindings in the `futures-mio` crate. This crate
//! is also an empty crate on Windows, as Unix Domain Sockets are Unix-specific.

// NB: this is all *very* similar to TCP/UDP, and that's intentional!

#![cfg(unix)]
#![doc(html_root_url = "https://docs.rs/tokio-uds/0.1")]
#![deny(missing_docs, warnings, missing_debug_implementations)]

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
mod recv_dgram;
mod send_dgram;
mod stream;
mod ucred;

pub use datagram::UnixDatagram;
pub use incoming::Incoming;
pub use listener::UnixListener;
pub use recv_dgram::RecvDgram;
pub use send_dgram::SendDgram;
pub use stream::UnixStream;
pub use ucred::UCred;

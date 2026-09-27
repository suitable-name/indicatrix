//! Coordinator tests: [`registry`] (check-out, notifications, the `WELCOME` advertisement --
//! no sockets beyond loopback pairs) and [`e2e`] (a real coordinator on ephemeral
//! loopback ports with real TLS, `join`, enrollment tokens and liveness); [`jobs`],
//! [`pictures`] and [`hdr`] run requests through it (the last: HDR maps forwarded to
//! joined workers).

mod e2e;
mod fixtures;
mod hdr;
mod jobs;
mod pictures;
mod registry;
mod support;

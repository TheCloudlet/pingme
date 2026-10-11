//! The Host side of pingme: the `LocalServices` seam and its production
//! implementation, Agent Session launch, and the Host daemon.

pub mod daemon;
pub mod launch;
pub mod services;
pub mod socket;

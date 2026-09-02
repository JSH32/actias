//! An object's life on this node beyond the call itself: the blob
//! store that holds its shipped state and manifests, the shipper that
//! moves committed frames there behind the output gate, and the sweeper
//! that wakes objects whose alarms came due while they were cold.

pub mod shipper;
pub mod store;
pub mod sweeper;

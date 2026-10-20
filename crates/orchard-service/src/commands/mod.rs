//! One module per screen. Every command mirrors an entry of the `Commands`
//! table in `app/src/lib/api.ts`.

// Command parameters are the frontend contract plus injected service state, so
// task commands legitimately take more than seven.
#![allow(clippy::too_many_arguments)]

pub mod app;
pub mod apps;
pub mod backups;
pub mod console;
pub mod devices;
pub mod files;
pub mod media;
pub mod photos;
pub mod sign;
pub mod store;

//! Private implementation and narrow executable seams for Stackmap.
//!
//! Stackmap is distributed as a binary. The supported contracts are the CLI,
//! configuration, documented interaction behavior, and release artifacts.
//!
//! The executable facade is available through [`runtime`]. Former implementation
//! module paths are intentionally unavailable:
//!
//! ```compile_fail
//! use stackmap::app::App;
//! ```
//!
//! ```
//! use stackmap::runtime::App;
//! let _app = App::default();
//! ```

#![deny(missing_docs)]

#[allow(missing_docs)]
mod adapters;
#[allow(missing_docs)]
mod app;
#[cfg(feature = "benchmarking")]
mod benchmark_impl;
#[allow(missing_docs)]
mod config;
#[allow(missing_docs)]
mod events;
#[allow(missing_docs)]
mod model;
#[allow(missing_docs)]
mod refresh;
#[allow(missing_docs)]
mod ui;

/// The deliberately small API used by the `stackmap` executable.
///
/// Stackmap is a binary-first application. These exports connect the thin
/// process/terminal shell to private implementation modules; they are not a
/// general-purpose Git topology library.
pub mod runtime {
    pub use crate::app::{Action, App, ConfigWriteRequest};
    pub use crate::config::{Config, ConfigMutation};
    pub use crate::events::{Input, Key};
    pub use crate::model::topology::ArchiveMode;
    pub use crate::model::{
        Branch, BranchId, ConfiguredUpstream, DiffState, GraphiteProvenance, RemoteRefEvidence,
        RepositorySnapshot, RepositoryState,
    };
    pub use crate::refresh::{RefreshEvent, RefreshHandle};
    pub use crate::ui::render;

    /// Exact local Git operations used by the executable shell.
    pub mod git {
        pub use crate::adapters::git::{DeleteOutcome, DeleteRequest, GitAdapter};
    }

    /// Optional GitHub pull-request enrichment.
    pub mod github {
        pub use crate::adapters::github::fetch;
    }

    /// Bounded macOS open and clipboard helpers.
    pub mod platform {
        pub use crate::adapters::platform::{copy_text, open_url};
    }
}

/// The intentionally tiny entrypoint used by the external benchmark target.
#[cfg(feature = "benchmarking")]
pub mod benchmark {
    /// Runs the deterministic topology/projection benchmark suite.
    pub fn run() {
        crate::benchmark_impl::run();
    }
}

#[cfg(test)]
mod integration_tests;

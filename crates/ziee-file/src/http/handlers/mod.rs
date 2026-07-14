//! The store-generic file HTTP handlers moved from ziee's `modules/file/handlers`
//! (chunk `ziee-file-http`). Only the handlers that depend on nothing beyond the
//! store ([`crate::repository::FileRepository`] via [`super::context::FileContext`]),
//! the storage manager, the injected [`crate::seams::FileEvents`], and the
//! download-token signer moved here; the processing/pandoc/identity-recheck
//! handlers stayed ziee-side (see [`super`]).

pub mod download;
pub mod management;
pub mod versions;

pub use download::*;
pub use management::*;
pub use versions::*;

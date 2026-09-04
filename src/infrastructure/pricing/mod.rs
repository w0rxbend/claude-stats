//! Where a run's price sheet comes from.
//!
//! Two pieces: a Gateway over an optional file the user may have written
//! ([`overrides`]), and the [`crate::application::ports::PriceSheetSource`]
//! implementation that composes it with the compiled-in sheet
//! ([`source`]).
//!
//! # What is deliberately not built here, and why
//!
//! There is no fetch of an upstream price list -- not `LiteLLM`'s, not
//! `models.dev`'s -- and no on-disk cache of fetched prices. **The tool never
//! opens a socket.** That is a decision, not an omission, and it rests on
//! three things:
//!
//! * The compiled-in sheet is maintained in-tree, reviewed like any other
//!   change, and correct. A wrong price is a bug, and the fix for a bug is a
//!   release -- not a request made on every invocation by every user for ever.
//! * A command-line tool that reaches out over the network when you ask it
//!   what you spent is a surprise. It is also a failure mode: it can hang on a
//!   captive portal, it can be slower than the report it is printing, and it
//!   turns "what did I spend" into a question that needs connectivity to
//!   answer.
//! * A fetched sheet makes yesterday's report unreproducible. Two runs a week
//!   apart would silently disagree, which is precisely the problem
//!   [`crate::domain::pricing::Provenance`] exists to make visible.
//!
//! The port is still worth having. It means a future gateway -- if one is ever
//! genuinely wanted -- plugs in at the composition root without touching the
//! domain, the reports or the dashboard. That benefit is available now, from
//! the separation alone, without shipping any network code to get it. Users
//! who need a rate corrected today have the override file below, which is
//! faster than a fetch and does not depend on anyone else's uptime.

pub mod overrides;
pub mod source;

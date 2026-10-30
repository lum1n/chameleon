//! chameleon's app model. Sans-IO like acc-core: [`App::handle`] takes
//! [`AppEvent`]s and returns [`Effect`]s for the runtime; [`App::screen`]
//! describes one frame with no terminal types in it.

pub mod auto;
pub mod composer;
pub mod config;
pub mod diff;
pub mod event;
pub mod hooks;
pub mod image;
pub mod keymap;
pub mod loadout;
pub mod markdown;
pub mod model;
pub mod overlay;
mod pump;
pub mod select;
pub mod slot;
pub mod style;
pub mod themes;
pub mod usage;
pub mod view;
pub mod wrap;

pub use event::{AppEvent, Effect, ExecOutput, ExecTag, HostId};
pub use model::{App, Pane};
pub use slot::SessionKey;
pub use view::Screen;

/// Milliseconds since the Unix epoch, for `now_ms` arguments.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

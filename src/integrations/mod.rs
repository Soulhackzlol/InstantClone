//! Integrations: react to what happens in InstantClone (a crash, a cut, a
//! chat command) by posting to Discord or Twitch chat, pushing to a phone,
//! calling a web service, running a program, or acting on the stream.
//!
//! Layout:
//! - `event`: what can happen, with the variables each event carries.
//! - `model`: integrations, their triggers and steps, and how they save.
//! - `presets`: the catalog of ready-made integrations and packs.
//! - `template`: `{var}` / `{var|fallback}` message templates.
//! - `botcmd`: Nightbot / StreamElements / Fossabot / Streamlabs commands
//!   converted into a "command from a website".
//! - `runner`: runs one handler's steps against a `host::Host`.
//! - `alerts`: what "Show on stream" steps put on the alerts page in OBS.
//! - `host` / `effects`: the live state and I/O a run needs, and the real
//!   implementation of both.
//! - `engine`: the isolated thread that matches events to integrations.
//! - `twitch`: login, token upkeep, chat, markers and clips.
//! - `store`: Twitch logins and counters, kept apart from the settings.

pub mod alerts;
pub mod api;
pub mod botcmd;
pub mod clock;
pub mod effects;
pub mod engine;
pub mod event;
pub mod host;
pub mod model;
pub mod presets;
pub mod recipe;
pub mod runner;
pub mod store;
pub mod template;
pub mod twitch;

pub use engine::Handle;
/// Only the tray (hotkeys) and the MIDI listener press shortcuts: Windows.
#[cfg(windows)]
pub use engine::Shortcut;
pub use event::{Event, EventKind};

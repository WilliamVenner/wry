// Copyright 2019-2021 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use crate::WindowProxy;

/// An event enumeration sent to [`FileDropHandler`].
#[derive(Debug, Serialize, Clone)]
pub enum FileDropEvent {
  /// The file(s) have been dragged onto the window, but have not been dropped yet.
  Hovered(FileDropData),
  /// The file(s) have been dropped onto the window.
  Dropped(FileDropData),
  /// The file drop was aborted.
  Cancelled,
}

#[derive(Debug, Serialize, Clone)]
/// The type of data that was dropped onto the webview.
pub enum FileDropData {
  /// A list of file paths.
  Paths(Vec<PathBuf>),

  /// A valid UTF-8 string.
  /// Some binary data is actually valid UTF-8, so this may be unexpected in some cases.
  Unicode(String),

  /// Raw binary data that could not be converted to UTF-8.
  Binary(Vec<u8>)
}

/// A listener closure to process incoming [`FileDropEvent`] of the webview.
///
/// Users can pass a [`WindowFileDropHandler`] to [`Application::add_window_with_configs`](crate::Application::add_window_with_configs)
/// to register incoming file drop events to a closure.
///
/// # Blocking OS Default Behavior
/// Return `true` in the callback to block the OS' default behavior of handling a file drop.
///
/// Note, that if you do block this behavior, it won't be possible to drop files on `<input type="file">` forms.
/// Also note, that it's not possible to manually set the value of a `<input type="file">` via JavaScript for security reasons.
///
/// # Example
///
/// ```no_run
/// use wry::{Application, Result, WindowProxy, FileDropEvent};
///
/// fn main() -> Result<()> {
///     let mut app = Application::new()?;
///     let file_drop = Box::new(|window: WindowProxy, event: FileDropEvent| {
///       // Use the `WindowProxy` to modify the window, eg: `set_fullscreen` etc.
///       //
///       // Use the `FileDropEvent` to see the current state of the file drop.
///       //
///       // Return `true` to block the default file drop behavior of the OS.
///       false
///     });
///     app.add_window_with_configs(Default::default(), None, vec![], Some(file_drop))?;
///     app.run();
///     Ok(())
/// }
/// ```
pub type WindowFileDropHandler = Box<dyn Fn(WindowProxy, FileDropEvent) -> bool + Send>;

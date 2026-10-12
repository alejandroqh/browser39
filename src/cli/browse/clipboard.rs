//! Copy text to the system clipboard: the native clipboard first (Windows
//! API, pbcopy, wl-copy, xclip/xsel), then the OSC 52 terminal escape, which
//! also works over SSH in terminals that support it.

use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Copy `text`, returning the name of the mechanism that accepted it.
pub fn copy(text: &str) -> io::Result<&'static str> {
    #[cfg(windows)]
    if clipboard_win::set_clipboard_string(text).is_ok() {
        return Ok("Windows clipboard");
    }
    for (cmd, args) in native_tools() {
        if pipe_to(cmd, args, text).is_ok() {
            return Ok(cmd);
        }
    }
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
    out.flush()?;
    Ok("OSC 52")
}

fn native_tools() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }
    if cfg!(windows) {
        // Handled by the clipboard API above (clip.exe would mangle UTF-8)
        return vec![];
    }
    let mut tools: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.push(("wl-copy", &[]));
    }
    if env::var_os("DISPLAY").is_some() {
        tools.push(("xclip", &["-selection", "clipboard"]));
        tools.push(("xsel", &["--clipboard", "--input"]));
    }
    tools
}

fn pipe_to(cmd: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{cmd} exited with {status}")))
    }
}

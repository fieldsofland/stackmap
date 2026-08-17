use std::ffi::OsStr;
use std::time::Duration;

use anyhow::{Result, bail};

use super::command::{run_bounded, run_bounded_with_stdin};

const PLATFORM_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_LIMIT: usize = 64 * 1024;

pub fn open_url(url: &str) -> Result<()> {
    open_urls(&[url])
}

pub fn open_urls(urls: &[impl AsRef<str>]) -> Result<()> {
    if urls.is_empty() {
        return Ok(());
    }
    let cwd = std::env::current_dir()?;
    let arguments: Vec<&str> = urls.iter().map(|url| url.as_ref()).collect();
    let output = run_bounded(
        OsStr::new("open"),
        arguments,
        &cwd,
        PLATFORM_TIMEOUT,
        OUTPUT_LIMIT,
    )?;
    if !output.status.success() {
        bail!(
            "URL opener exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let output = run_bounded_with_stdin(
        OsStr::new("pbcopy"),
        std::iter::empty::<&str>(),
        &cwd,
        PLATFORM_TIMEOUT,
        OUTPUT_LIMIT,
        text.as_bytes(),
    )?;
    if !output.status.success() {
        bail!(
            "clipboard helper exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}

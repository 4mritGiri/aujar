use crate::Rgb;
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "linux")]
use ashpd::desktop::Color;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// How long to wait for the user to pick (or cancel) before giving up.
#[cfg(target_os = "linux")]
const PICK_TIMEOUT: Duration = Duration::from_secs(120);

/// Let the user pick a pixel on screen through the desktop's color picker
/// (`org.freedesktop.portal.Screenshot.PickColor`). Resolves when the user picks
/// a color or cancels.
///
/// The portal call runs on a blocking thread with its own runtime, so callers
/// do not depend on the portal's futures being `Send`.
pub async fn pick() -> Result<Rgb> {
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("the native color picker backend is not implemented for this platform yet");

    #[cfg(target_os = "linux")]
    {
        tokio::task::spawn_blocking(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to start a runtime for the color picker")?;

            runtime.block_on(async {
                tokio::time::timeout(PICK_TIMEOUT, pick_inner())
                .await
                .context(
                    "timed out waiting for a color to be picked (did the picker overlay appear?)",
                )?
            })
        })
        .await
        .context("the color picker task failed")?
    }
}

#[cfg(target_os = "linux")]
async fn pick_inner() -> Result<Rgb> {
    let color = Color::pick()
        .send()
        .await
        .context("the color picker portal is not available on this desktop")?
        .response()
        .context("color picking was cancelled or failed")?;

    Ok(Rgb::from_unit(color.red(), color.green(), color.blue()))
}

use crate::Rgb;
use anyhow::{Context, Result};
use ashpd::desktop::Color;

/// Let the user pick a pixel on screen through the desktop's color picker
/// (`org.freedesktop.portal.Screenshot.PickColor`). Resolves when the user picks
/// a color or cancels.
///
/// The portal call runs on a blocking thread with its own runtime, so callers
/// do not depend on the portal's futures being `Send`.
pub async fn pick() -> Result<Rgb> {
    tokio::task::spawn_blocking(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to start a runtime for the color picker")?;

        runtime.block_on(pick_inner())
    })
    .await
    .context("the color picker task failed")?
}

async fn pick_inner() -> Result<Rgb> {
    let color = Color::pick()
        .send()
        .await
        .context("the color picker portal is not available on this desktop")?
        .response()
        .context("color picking was cancelled or failed")?;

    Ok(Rgb::from_unit(color.red(), color.green(), color.blue()))
}

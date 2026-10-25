//! Self-contained authorization page assets, using Studio's original tokens and icon.
use super::*;

pub(super) async fn asset(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    let (mime, bytes): (&str, &[u8]) = match name.as_str() {
        "style.css" => (
            "text/css; charset=utf-8",
            concat!(
                include_str!("../../../../packages/ui/src/foundation/tokens.css"),
                "\n",
                include_str!("device.css")
            )
            .as_bytes(),
        ),
        "appearance.js" => (
            "application/javascript; charset=utf-8",
            include_bytes!("device-appearance.js"),
        ),
        "icon.svg" => (
            "image/svg+xml",
            include_bytes!("../../../../packages/ui/src/assets/layrs-icon.svg"),
        ),
        "inter.woff2" => ("font/woff2", include_bytes!("fonts/inter.woff2")),
        "audiowide.woff2" => ("font/woff2", include_bytes!("fonts/audiowide.woff2")),
        "jetbrains-mono.woff2" => ("font/woff2", include_bytes!("fonts/jetbrains-mono.woff2")),
        "font-licenses.txt" => (
            "text/plain; charset=utf-8",
            concat!(
                include_str!("fonts/inter-LICENSE.txt"),
                "\n",
                include_str!("fonts/audiowide-LICENSE.txt"),
                "\n",
                include_str!("fonts/jetbrains-mono-LICENSE.txt")
            )
            .as_bytes(),
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
}

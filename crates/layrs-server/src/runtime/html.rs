use super::RuntimeConfig;

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn server_page(config: &RuntimeConfig) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Layrs Server</title>
    <style>
      :root {{
        color-scheme: light dark;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        line-height: 1.5;
      }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: Canvas;
        color: CanvasText;
      }}
      main {{
        width: min(760px, calc(100vw - 40px));
      }}
      h1 {{
        font-size: 28px;
        margin: 0 0 8px;
      }}
      p {{
        margin: 0 0 20px;
        color: color-mix(in srgb, CanvasText 72%, Canvas 28%);
      }}
      dl {{
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 10px 16px;
        padding: 18px 0;
        border-top: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
        border-bottom: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
      }}
      dt {{
        font-weight: 700;
      }}
      a {{
        color: LinkText;
      }}
      code {{
        font: inherit;
        font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
      }}
    </style>
  </head>
  <body>
    <main>
      <h1>Layrs Server</h1>
      <p>Local development server for API contracts, health checks, auth and Studio handoff.</p>
      <dl>
        <dt>Status</dt>
        <dd><a href="/healthz">/healthz</a></dd>
        <dt>Routes</dt>
        <dd><a href="/v1/routes">/v1/routes</a></dd>
        <dt>Studio Web</dt>
        <dd><a href="{studio_url}">{studio_url}</a></dd>
        <dt>Runtime</dt>
        <dd><code>std-http</code>, auth store <code>dev-memory</code>, database configured: <code>{database_url_configured}</code></dd>
      </dl>
      <p>This page is intentionally a server status surface. The full product interface runs in Studio Web.</p>
    </main>
  </body>
</html>"#,
        studio_url = escape_html(&config.studio_url),
        database_url_configured = config.database_url_configured()
    )
}

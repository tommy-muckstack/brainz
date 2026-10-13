# Brainz network behavior

Brain files and canvases are local. Personal API keys remain optional and are
stored through the app's credential provider. These notes describe the current
implementation and checks, not a completed packet-capture audit.

| Activity | Destination | Trigger |
| --- | --- | --- |
| App updates | Brainz's public download store | Packaged app's hourly check, or manual Check for updates |
| Installer download | HTTPS URL in the Brainz feed | A newer eligible release, followed by checksum and native signature verification |
| Installed release notes | None | Bundled changelog opens in a Markdown tab |
| Legacy Zed telemetry | None | No destination is configured, including with stale `metrics: true` settings |
| Crash uploads | Explicit Brainz endpoint only | Diagnostics opt-in and configured endpoint; off by default |
| Zed account authentication | Zed services | Explicit sign-in; Brainz skips automatic startup authentication |
| Extension catalog | Zed extension service | Explicit browsing or update request; Brainz skips startup update checks |
| Extension installation | Extension publisher and registry hosts | User-installed extension or explicit auto-install preference; bundled defaults install none |
| Language tools | Tool publisher hosts | A configured language server, formatter, or extension needs its tools |
| Claude/Codex chat | Configured assistant provider and connectors | User starts the connected assistant or sends a message |
| Personal-key insights | Selected OpenAI or Anthropic provider | Optional key is available and the relevant insight feature runs |
| Connector OAuth | Connector provider | User connects or reconnects |
| Git synchronization | Configured Git remote | User syncs or enables an explicit synchronization workflow |

Inherited usage telemetry cannot send requests even when an older settings file
has opted into Zed metrics. A regression test uses an HTTP client that panics on
any request and checks both default and stale opt-in settings. Default metrics
and diagnostics are both false. Inherited `ZED_MINIDUMP_ENDPOINT` is ignored.

Edit predictions default to `none`. Automatic extension update checks and stored
Zed account authentication are skipped for Brainz's internal Dev channel.
Explicitly configured tools and extensions can still make their own requests.

A ten-minute network capture covering notes, PDF, canvas, and a Claude tab is
still pending. Do not describe that acceptance check as complete until its
results are recorded here.

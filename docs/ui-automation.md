# GPUI UI capture lab

Jira Desk has a development-only, macOS-only screenshot lab for deterministic
visual iteration. It renders named Jira Desk fixture scenarios through GPUI's
offscreen Metal capture API and writes PNG files; it does not use shell
`screencapture`, coordinate automation, or production startup code.

## Run it

```bash
cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --scenario issues --output target/ui-lab/issues-light.png \
  --size 1280x900 --theme light

cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --scenario settings --output target/ui-lab/settings-dark.png \
  --theme dark
```

Use `--help` for all options and `--list` for the supported semantic scenarios:
`onboarding`, `issues`, `updates`, `team`, and `settings`. `--size` is a
logical window size; the command reports the physical PNG dimensions returned
by the renderer. The output directory is created when needed.

The fixture scenarios are explicit, stable constructions in the GPUI adapter.
They reuse the existing sample data and never initialize live Jira workspaces,
credential or keychain loading, network clients, polling, persistence,
downloads, notifications, or Jira write ports. Captures are not baselines and
this milestone intentionally does not add or update baseline images.

The lab is intentionally separate from the normal `jira-gpui` binary and macOS
DMG packaging. Build it only during development with `--features ui-lab`.
Linux can type-check the feature, but capture execution reports a clear macOS-only
error.

Accessibility (AX) and real-window smoke tests are a later layer. This first
milestone validates the GPUI-native rendered surface without requiring a visible
window or coordinate-driven interaction. Do not put credentials, keychain data,
Jira URLs, or generated PNGs in the repository.

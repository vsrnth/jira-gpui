# UI component audit

This is the concise inventory for `gpui-component` adoption in the supported
native GPUI shells: Linux x86_64 on Wayland and native macOS arm64/x86_64. The
goal is semantic behavior and keyboard/accessibility correctness, not replacing
working custom controls merely to increase library usage.

## Current baseline

The shell already uses or intentionally composes on both supported platforms:

- `Root`, `TitleBar`, and `Resizable` for the application frame and list/detail
  split;
- `Input` and `Combobox` for onboarding, local search, and status filters;
- `Notification` for in-app refresh/comment outcomes; on Linux, Freedesktop
  alerts remain independent, while the adapter/test is unavailable on macOS;
- the registered `gpui-component-assets` bundle for TitleBar and semantic icon
  assets, keeping idle minimize/maximize/close controls discoverable while
  retaining hover emphasis;
- custom ADF rendering, responsive mobile navigation, and the existing detail
  presentation where domain-specific semantics matter.

The current batch adds the low-risk primitives that remove duplicated state or
make existing interaction state visible: `Spinner`, component scrollbars,
`Button`, and explicit `aria`/accessibility labels and states. A loading button
must expose disabled/loading semantics, and scrollbars must not change the
bounded list/detail ownership model.

## Prioritized follow-up

### P1

- `DescriptionList`: use for stable issue metadata rows when its label/value
  semantics preserve responsive wrapping and display-name-only identity rules.
- `AlertDialog`: use for destructive-looking or consequential confirmations,
  especially explicit attachment download destination/cancel/error flows and
  confirmed comment creation, only where focus trapping and keyboard dismissal
  are correct.
- `SidebarMenu`: use if the current responsive navigation rail gains more
  destinations; retain the current mobile back-navigation semantics.

### P2

- `Tag`/`Badge`: consider for status, issue type, and priority only after color,
  contrast, and text semantics remain sufficient without relying on color.
- List virtualization: profile the issue list first. Introduce virtualization
  only if measurement shows a real performance problem; preserve keyboard
  focus, selection, bounded stale-result behavior, and scrollbar semantics.

## Explicitly rejected for now

- `DataTable`: the issue list has row selection and responsive detail ownership,
  not table headers/cell navigation. Reconsider only with real tabular
  semantics and a demonstrated need.
- `Tabs`: Issues and Updates are navigation destinations, not panels that share
  a tabbed detail context. Reconsider only when separate panels need tab
  keyboard semantics.
- `Avatar`: the product intentionally renders display names and safe fallback
  labels, not remote avatar URLs. Reconsider only with a real identity,
  privacy, and image-loading requirement.

## Version and toolchain constraints

The repository is pinned to the verified `gpui-component` HEAD
`b29ee13379e161c2fb68c14c229c958d52d6ffe4` (package `0.5.2`); Cargo.lock
resolves its compatible GPUI/Zed revision to
`cc053a4a6fa2fd0e8793201ed9099466af1be0b1`. Keep those revisions aligned
through the lockfile rather than adding a second GPUI source identity. Rust
1.95 remains sufficient for the
current component set; Rust 1.97.1 is not required. Any upgrade must preserve
native GPUI support on Linux Wayland and macOS, and rerun the platform-specific
release smoke. Linux smoke includes media cancellation, XDG portal download,
and independent Freedesktop alert checks; macOS smoke uses its native file
picker and does not require the unavailable Freedesktop adapter/test. The media
smoke should include an unresolved ADF Media Services UUID,
the bounded labeled fallback gallery, thumbnail-404 and precise unknown-MIME/
unrecognized-signature thumbnail-unavailable original-content fallback, and
byte-signature format selection after MIME allowlists. OS alerts remain
unchanged.

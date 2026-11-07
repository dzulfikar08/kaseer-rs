# Kaseer in the Atur design language

Kaseer (desktop register, web register, cloud console, landing site) follows
Atur Terminal's design language. Source of truth for the values is
atur-terminal `crates/muxy-ui` (bundled "Atur" / "Atur Light" schemes,
`theme/colors.rs`, `theme/metrics.rs`, `theme/elevation.rs`, `controls.rs`).
The desktop implementation is `kaseer-rs/app/src/theme.rs`.

This replaces the 2026-09 brand board look (Midnight + navy surfaces,
Electric Blue accent, K gradient, Plus Jakarta Sans).

## Principles

- **Neutral monochrome.** No hue in the chrome. Every neutral is the text
  colour at a fixed alpha over the canvas.
- **The accent is light gray (dark) / near-black (light)**, with the
  contrasting colour on it. It marks state and the one primary action per
  view: the active nav item, the active chip/tab, focus, Pay/Save. It never
  decorates.
- **Colour only carries information**: danger, ok, warning states, and the
  register's category hues (a product's category edge and its chip dot).
- **No gradients, no glows, no coloured shadows.** Depth comes from
  hairline borders and one layered neutral shadow on floating layers.
- **Compact and quiet.** Small type, medium weight for emphasis (semibold
  only for the one figure that matters: a total, a KPI value), tight
  spacing, 24px controls.

## Dark palette ("Atur")

| Token | Value | Derivation | Use |
|---|---|---|---|
| `bg` | `#0c0c0c` | scheme background | canvas |
| `rail` | `#111111` | canvas +3% | nav rail, key/status bar, table head |
| `surface` | `#1e1e1e` | fg @ 8% | panels, cards, tiles, secondary buttons |
| `surface-2` / hover | `#2a2a2a` | fg @ 6% over surface | hovered rows, nested blocks |
| `border` | `#272727` | fg @ 12% | hairlines, panel borders |
| `border-strong` | `#383838` | | input borders, outlined chips, key caps |
| `fg` | `#ebebeb` | scheme foreground | text |
| `fg-muted` | `#9d9d9d` | fg @ 65% | secondary text, labels |
| `fg-dim` | `#656565` | fg @ 40% | captions, counters, key legends only |
| `accent` | `#e4e4e4` | `atur-accent` | primary action, active state |
| `accent-hover` | `#f5f5f5` | | |
| `on-accent` | `#000000` | contrasting fg | text on accent |
| `accent-soft` | `rgba(228,228,228,.10)` | accent @ 10% | active chip fill, selected row |
| `danger` | `#e07070` | ANSI 1 | |
| `ok` | `#6fbf8a` | ANSI 2 | |
| `warning` | `#d4b062` | ANSI 3 | |
| selection | `rgba(235,235,235,.22)` | | text selection |
| overlay | `rgba(0,0,0,.6)` | | modal scrim |

## Light palette ("Atur Light")

For the marketing site (which stays light) and any light mode.

| Token | Value |
|---|---|
| `bg` | `#fafafa` |
| `surface` | `#efefef` (fg @ 6%) |
| `surface-2` | `#e6e6e6` |
| `border` | `#e0e0e0` (fg @ 12%) |
| `border-strong` | `#cfcfcf` |
| `fg` | `#171717` |
| `fg-muted` | `#5f5f5f` (fg @ 65%) |
| `fg-dim` | `#8a8a8a` |
| `accent` | `#171717`, `on-accent` `#fafafa` |
| `danger` | `#b3373b` |
| `ok` | `#2f7d4f` |
| `warning` | `#8f6a1c` |

## Category hues (register only)

In category order, then a stable FNV-1a hash of the name for unknown ones:
`#d4b062` ochre, `#e07070` red, `#6fbf8a` green, `#6fb8c4` cyan,
`#b595c4` magenta, `#7d9fd0` blue. (Light: `#8f6a1c #b3373b #2f7d4f
#1f7683 #7a4f93 #3f66a8`.)

## Type

- UI: **Inter** (self-hosted; Atur draws the system UI face, a kiosk cannot
  rely on one). Weights 400 / 500 / 600.
- Mono: **IBM Plex Mono** for key caps, metadata, counters, IDs, receipt.
- Money: Inter with `font-variant-numeric: tabular-nums` (`tnum`).
- Scale (px): caption 10, footnote 11, **body 12**, emphasis 13,
  headline 14, title 16, display 20, hero 24, mega 28. Base UI size 13.
- Section titles: 11px, weight 500, `fg-muted`, uppercase, letter-spacing
  .04em. Not bold, not large.
- Line height: 16 compact, 18 fields.

## Spacing, size, shape

- Spacing steps: 2 4 6 8 10 12 16 20 24 32.
- Controls: small 20, **medium 24**, large 32 (inputs 28-32 on touch-first
  screens; the register's Pay is 44).
- Radius: sm 4 (key caps, badges), **md 6 (buttons, inputs)**, lg 8
  (cards, panels, tiles), xl 10 (modals, popovers). Chips and toggles are
  full pills.
- Borders: 1px hairline `border`. Inputs use `border-strong`; focus turns
  the border `fg-muted` (no glow ring).
- Bars: tab bar 28, header 36, title bar 32, status/key bar 28.
- Shadow (floating layers only, Atur `Elevation::Modal`):
  `0 2px 3px rgba(0,0,0,.12), 0 3px 6px rgba(0,0,0,.08),
  0 6px 12px rgba(0,0,0,.04), 0 1px 0 rgba(0,0,0,.12)`.

## Components

- **Primary button**: accent fill, on-accent text, weight 500, hover
  accent-hover. One per view.
- **Secondary button** (Atur `controls::button`): surface fill, 1px border,
  fg text, 11-12px weight 500, hover surface-2.
- **Ghost button**: no fill, fg-muted text, hover surface-2.
- **Toggle**: 28x16 pill; off = surface + border, knob fg-muted; on =
  accent fill, knob on-accent.
- **Chip**: pill, 24-28px tall. Inactive = border-strong outline, fg-muted;
  active = accent-soft fill + accent border + fg text, weight 500.
- **Panel / card**: surface, 1px border, radius 8, padding 12-16.
- **KPI tile**: caption title (section-title style), value 20-24px weight
  600 tabular.
- **Table**: head on rail colour, 11px uppercase fg-muted; rows separated by
  hairlines; hover surface-2; numbers right-aligned tabular.
- **Nav rail**: rail colour, icon + small label, fg-muted; active item =
  surface fill + fg text. No coloured active state.
- **Key bar / status bar**: 28px, rail colour, mono key caps
  (border-strong outline, radius 4, 10px) beside 11px labels.
- **Brand mark**: the K-mark drawn flat in the accent colour (no gradient).

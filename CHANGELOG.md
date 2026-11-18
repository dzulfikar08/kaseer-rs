# Changelog

All notable changes to Kaseer are listed here. Versions follow [Semantic Versioning](https://semver.org).

## [Unreleased]

### Added

- Profit guard: a whole-sale discount never takes the sale under the owner's minimum profit over
  HPP (cost of goods, default 10% of the sale). The discount dialog greys out the presets past
  the limit and says how far this cart can go; a bigger discount, or a win-back code, is lowered
  to the limit and the cart says so. Adding a high-margin item gives the discount back. A code
  that would leave no discount at all is refused, so it stays unused. Products without an HPP
  are not checked. Settings has the switch and the percentage; with Kaseer Cloud they follow the
  owner's Grow settings.

## [0.5.0] - 2026-11-17

The register now helps sell: it offers the usual add-on, takes win-back codes, and turns walk-ins
into regulars. With Kaseer Cloud, the owner sees what each of these brought in.

### Added

- Add-on button: after a product is added, the cart offers what customers usually take with it
  ("Ask: 36% of Kopi Susu orders take this too", then one big button or No). Pairs come from
  Kaseer Cloud, or from this register's last 30 days of sales without a cloud. Each sale records
  which add-ons were offered and taken, so the cloud measures the real yes-rate.
- Win-back codes: F3 (or the Code button) takes the code from the customer's WhatsApp message,
  attaches the customer and takes the discount; a customer with a live code gets an Apply prompt.
  Codes arrive with the menu, so they work offline; a code is used once. Changing the customer or
  the discount takes the code off the sale.
- WhatsApp number at payment (optional): finds the customer or adds a new one, so a walk-in earns
  points and can be won back later. A mistyped number never holds up the sale.
- Receipts on WhatsApp for customers with a number.
- Settings: switches for the add-on button and the number prompt (with Kaseer Cloud they follow
  the owner's Grow settings).

### Changed

- The "Often bought with" card is replaced by the add-on button.
- Ledger schema v5 (shared with the web companion): `sales.offer_code`,
  `sales.addon_offers_json` and the `winback_offers` table.

## [0.4.1] - 2026-11-17

Shift, Inventory and Customers get the register's big-button treatment.

### Changed

- Shift screen sized for touch: a big opening-float field with Rp 50.000 / 100.000 / 500.000
  buttons and an 84px Open shift button; a 64px Close shift button; closing the drawer counts
  each banknote with − / + buttons.
- Inventory sized for touch: a big search bar (product, SKU or category), 56px All / Low / Out
  buttons, taller rows with the on-hand count in large type, and the whole row opens the stock
  dialog. The dialog has three big buttons, Receive, Remove and Count: type or tap the quantity
  with − / + and quick +1 / +6 / +12 / +24 buttons, and it shows the result ("On hand 40 → 64")
  before you apply. Count books the difference to what is on the shelf.
- Customers sized for touch: a big search bar, larger rows with name, phone and join date,
  52px − / + buttons for points, a big Sell to button, and a roomier Add customer form.
- Releases also carry fixed-name copies (`Kaseer.dmg`, `Kaseer-Setup-x64.exe`, `Kaseer-x64.zip`,
  `Kaseer-<arch>.AppImage`), so `releases/latest/download/<name>` always serves the newest build.

## [0.4.0] - 2026-11-17

The register is redesigned for touch screens: big buttons everywhere a cashier taps.

### Changed

- Register: larger product tiles (18px names, count badge when in the cart), 56px category
  buttons, a 60px scan bar with the shift status beside it.
- Cart: each line has big − / + buttons (− at one removes the line); Discount opens preset
  percentages (5–50%) or a custom one; Clear asks for a second tap; the Pay button shows the
  total and is 84px tall.
- Payment is a large screen instead of a small dialog: pick Cash, Card, QRIS or Transfer, tap
  Exact or a suggested cash amount (the next round amounts above the bill) or type it on a big
  number pad, and one button says what it will do ("Complete sale · change Rp 17.000"). An amount
  short of the total stages a partial payment, so split payments take two taps. Enter completes.
- Side rail: bigger items; Training, Knowledge and Settings grouped at the bottom.
- Receipt: big New sale button.
- macOS builds are signed with a Developer ID and notarised.

## [0.3.0] - 2026-11-17

First open-source release, under AGPL-3.0-or-later.

### Added

- Builds for Windows (installer and portable zip), macOS (universal DMG) and Linux (AppImage and
  tarball, x86_64 and aarch64), published by GitHub Actions on every `v*` tag.
- `kaseer --version`.
- App icon.

### Changed

- GPUI Kit 0.9. Dialogs, sheets and notifications are now mounted by the window root.
- The default cloud is `https://cloud.kaseer.site` (still editable in Settings, or with
  `KASEER_CLOUD_URL`).
- Data lives in the per-user data folder (`~/.local/share/kaseer`,
  `~/Library/Application Support/Kaseer`, `%APPDATA%\Kaseer`) unless `./data/kaseer.db` already
  exists or `KASEER_DATA_DIR` is set.
- Auto-update can replace the running binary on Windows, and comes from the latest GitHub release
  through Kaseer Cloud. AppImages skip self-update.

### Fixed

- F2 no longer opens payment on an empty cart or with no shift open.

## [0.2.5] - 2026-11-07

- Register restyled in Atur Terminal's monochrome design language; Inter and IBM Plex Mono.
- UI polish; F-key screen switching survives focus loss.

## [0.2.4]

- Global keyboard navigation: F-key screen switching, Alt+[ / Alt+] to cycle, F2 to pay.

## [0.2.0]

- Cloud kiosk: enrollment, device sync, verified auto-update.
- Menu pull from the companion, training mode, product knowledge, offline-first sync outbox.

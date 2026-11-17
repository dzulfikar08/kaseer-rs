# Changelog

All notable changes to Kaseer are listed here. Versions follow [Semantic Versioning](https://semver.org).

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

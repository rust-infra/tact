# Third-Party Notices

Tact binaries include third-party Rust crates distributed under their own
licenses. The primary UI dependencies are:

- Native Shell 0.3.0 (`nsh-core`, `nsh-base`, `nsh-engine`,
  `nsh-platform-macos`, `nsh-platform-linux`, `nsh-platform-windows`):
  Apache-2.0.
- IBM Plex Sans: SIL Open Font License 1.1. See
  `crates/desktop/assets/fonts/IBM-Plex-LICENSE.txt`.
- Lilex: SIL Open Font License 1.1. See
  `crates/desktop/assets/fonts/Lilex-LICENSE.txt`.
- Lucide icons (the composer's `send` glyph, copied into
  `crates/desktop/assets/icons/send.svg`): ISC.

The Apache License 2.0 text is included in
`THIRD-PARTY-APACHE-2.0.txt`. Release archives also include
`THIRD-PARTY-LICENSES.txt`, generated for the exact target from the Cargo
registry sources by `scripts/collect-licenses.py`, plus the `Cargo.lock` that
identifies every linked package and version.

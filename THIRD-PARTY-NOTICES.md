# Third-Party Notices

Tact binaries include third-party Rust crates distributed under their own
licenses. The primary UI dependencies are:

- `gpui-ai` 0.8.0 (local localization patch): MIT. See
  `vendor/gpui-ai/LICENSE`.
- `gpui-pre` 0.3.3: Apache-2.0.
- `gpui-component` 0.6.0: Apache-2.0.
- `gpui-base` 0.6.0: Apache-2.0.
- `gpui-kit-assets` 0.6.0: Apache-2.0.
- IBM Plex Sans: SIL Open Font License 1.1. See
  `crates/desktop/assets/fonts/IBM-Plex-LICENSE.txt`.
- Lilex: SIL Open Font License 1.1. See
  `crates/desktop/assets/fonts/Lilex-LICENSE.txt`.

The Apache License 2.0 text is included in
`THIRD-PARTY-APACHE-2.0.txt`. Release archives also include
`THIRD-PARTY-LICENSES.txt`, generated for the exact target from the Cargo
registry sources by `scripts/collect-licenses.py`, plus the `Cargo.lock` that
identifies every linked package and version.

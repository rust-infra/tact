//! In-app updates, driven by the release manifest the packaging pipeline writes.
//!
//! The shell ships one entry point ("Check for updates") and this module owns
//! the rest: ask the release's `latest.json` whether a newer version exists,
//! and if it does, verify its signature and install it. There is no second
//! confirmation, because the signature check is what makes the update safe:
//! the bundle must be signed with the key whose public half was compiled in,
//! so a tampered download fails before anything is written.
//!
//! Both halves of that contract are build-time constants rather than runtime
//! configuration. A desktop app that can be pointed at an arbitrary update
//! server by an environment variable is one `LD_PRELOAD`-style mistake away
//! from installing a stranger's bundle, so the endpoint and the key are baked
//! in by whoever builds the release.
//!
//! When no key was compiled in, the entry point says so instead of checking.
//! That is the honest state for a from-source build: there is no release whose
//! signature it could verify.

use std::time::Duration;

use cargo_packager_updater::UpdaterBuilder;

/// How long a check or a download may take before it gives up.
///
/// A hung update must not leave the entry point spinning forever, and the
/// caller is on a background thread where a stall is invisible.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(120);

/// Where the updater looks for the release manifest.
///
/// The default is the `latest.json` attached to the repository's newest
/// release; `TACT_UPDATE_ENDPOINT` overrides it at build time for a fork or a
/// staging channel.
pub(crate) fn endpoint() -> &'static str {
    option_env!("TACT_UPDATE_ENDPOINT")
        .unwrap_or("https://github.com/rust-infra/tact/releases/latest/download/latest.json")
}

/// The public half of the release signing key, or `None` for a from-source
/// build that has no release to verify against.
pub(crate) fn pubkey() -> Option<&'static str> {
    option_env!("TACT_UPDATE_PUBKEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

/// What a check-and-install run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The running version is the newest one the manifest announces.
    UpToDate { version: String },
    /// A newer version was downloaded, verified, and installed. The user has
    /// to restart for it to take effect.
    Installed { from: String, to: String },
    /// This build carries no public key, so it cannot verify a release.
    NotConfigured,
    /// Anything else, already rendered as a sentence fit for the transcript.
    Failed(String),
}

/// Check for an update and install it if there is one.
///
/// Blocking on purpose: the caller runs this on the background executor, and
/// every step of it — the request, the download, the signature check — is
/// blocking I/O.
pub(crate) fn check_and_install() -> Outcome {
    let Some(pubkey) = pubkey() else {
        return Outcome::NotConfigured;
    };
    let endpoint = match endpoint().parse::<url::Url>() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return Outcome::Failed(format!("the update endpoint is not a URL: {error}"));
        }
    };
    let current = env!("CARGO_PKG_VERSION");
    let version = match semver::Version::parse(current) {
        Ok(version) => version,
        Err(error) => {
            return Outcome::Failed(format!("this build's version is not semver: {error}"));
        }
    };

    let config = cargo_packager_updater::Config {
        endpoints: vec![endpoint],
        pubkey: pubkey.to_string(),
        ..Default::default()
    };
    let updater = match UpdaterBuilder::new(version, config)
        .timeout(UPDATE_TIMEOUT)
        .build()
    {
        Ok(updater) => updater,
        Err(error) => {
            return Outcome::Failed(format!("could not build the updater: {error}"));
        }
    };

    match updater.check() {
        Ok(None) => Outcome::UpToDate {
            version: current.to_string(),
        },
        Ok(Some(update)) => match update.download_and_install() {
            Ok(()) => Outcome::Installed {
                from: current.to_string(),
                to: update.version.clone(),
            },
            Err(error) => Outcome::Failed(format!(
                "could not install Tact {}: {error}",
                update.version
            )),
        },
        Err(error) => Outcome::Failed(format!("could not check for updates: {error}")),
    }
}

/// The sentence the transcript shows for an outcome.
pub(crate) fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::UpToDate { version } => {
            format!("Tact {version} is the newest release.")
        }
        Outcome::Installed { from, to } => {
            format!("Updated Tact from {from} to {to}. Restart the app to run the new version.")
        }
        Outcome::NotConfigured => {
            "This build carries no release signing key, so it cannot check for updates. \
             Install a released build to get automatic updates."
                .to_string()
        }
        Outcome::Failed(reason) => format!("Update failed: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outcome_reads_as_a_sentence() {
        assert_eq!(
            describe(&Outcome::UpToDate {
                version: "1.2.3".into()
            }),
            "Tact 1.2.3 is the newest release."
        );
        assert!(
            describe(&Outcome::Installed {
                from: "1.0.0".into(),
                to: "1.2.3".into()
            })
            .contains("1.0.0 to 1.2.3"),
            "both ends of the update are named"
        );
        assert!(
            describe(&Outcome::NotConfigured).contains("signing key"),
            "an unconfigured build says why it cannot check"
        );
        assert!(describe(&Outcome::Failed("boom".into())).contains("boom"));
    }

    #[test]
    fn the_build_facts_are_well_formed() {
        assert!(
            endpoint().starts_with("https://"),
            "the endpoint is https: {}",
            endpoint()
        );
        // A from-source build has no key; a release build has one. Either is a
        // valid state, but an empty string is not: it would look configured and
        // then fail every check.
        if let Some(key) = pubkey() {
            assert!(!key.trim().is_empty());
        }
    }
}

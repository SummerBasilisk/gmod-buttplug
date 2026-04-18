//! Best-effort "is there a newer release?" check against the GitHub Releases
//! API. Runs once on module load in a detached background thread; if the
//! latest published release's `tag_name` parses to a higher SemVer than
//! `CARGO_PKG_VERSION`, prints a one-line notice to the gmod console.
//!
//! Failures (offline, rate-limited, non-JSON response, unparseable tag) are
//! swallowed — we don't want to spam error output on user machines.

use std::thread;
use std::time::Duration;

const LATEST_RELEASE_URL: &str =
	"https://api.github.com/repos/SummerBasilisk/gmod-buttplug/releases/latest";
const RELEASES_PAGE: &str =
	"https://github.com/SummerBasilisk/gmod-buttplug/releases";

/// Spawn the background check. Detached — we never join the handle, so the
/// thread is free to outlive the caller; the process just cleans it up on
/// exit if it's still in flight.
pub fn spawn() {
	let _ = thread::Builder::new()
		.name("gmod-buttplug-update-check".into())
		.spawn(run);
}

fn run() {
	let current = env!("CARGO_PKG_VERSION");

	let agent = ureq::AgentBuilder::new()
		.timeout(Duration::from_secs(5))
		.user_agent(concat!(
			"gmod-buttplug/",
			env!("CARGO_PKG_VERSION"),
			" (+https://github.com/SummerBasilisk/gmod-buttplug)"
		))
		.build();

	let resp = match agent.get(LATEST_RELEASE_URL).call() {
		Ok(r) => r,
		Err(_) => return, // network error, rate-limit (403), 404, etc.
	};

	let body: serde_json::Value = match resp.into_json() {
		Ok(v) => v,
		Err(_) => return,
	};

	let tag = match body.get("tag_name").and_then(|v| v.as_str()) {
		Some(t) => t,
		None => return,
	};

	let latest = tag.trim_start_matches('v');
	if is_newer(latest, current) {
		println!(
			"[gmod-buttplug] update available: installed v{current}, latest v{latest}\n\
			 [gmod-buttplug]   {RELEASES_PAGE}"
		);
	}
}

fn is_newer(latest: &str, current: &str) -> bool {
	parse_version(latest) > parse_version(current)
}

/// Parse `major.minor.patch` to a tuple for lex comparison. Missing or
/// unparseable segments count as 0.
fn parse_version(s: &str) -> (u32, u32, u32) {
	let mut parts = s.split('.');
	let n = |p: Option<&str>| -> u32 {
		p.and_then(|x| x.split('-').next()) // strip any SemVer pre-release suffix
			.and_then(|x| x.parse().ok())
			.unwrap_or(0)
	};
	(n(parts.next()), n(parts.next()), n(parts.next()))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn newer_patch() {
		assert!(is_newer("0.2.1", "0.2.0"));
	}

	#[test]
	fn newer_minor() {
		assert!(is_newer("0.3.0", "0.2.9"));
	}

	#[test]
	fn newer_major() {
		assert!(is_newer("1.0.0", "0.99.99"));
	}

	#[test]
	fn equal_not_newer() {
		assert!(!is_newer("0.2.0", "0.2.0"));
	}

	#[test]
	fn older_not_newer() {
		assert!(!is_newer("0.1.9", "0.2.0"));
	}

	#[test]
	fn prerelease_suffix_treated_as_base() {
		// "0.2.1-rc1" parses as 0.2.1, which is newer than 0.2.0.
		assert!(is_newer("0.2.1-rc1", "0.2.0"));
	}
}

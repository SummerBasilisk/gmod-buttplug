//! Extracts the resolved `buttplug_client` version from `Cargo.lock` and
//! exposes it to the rest of the crate as the `BUTTPLUG_VERSION` env var,
//! so the module-load banner can print the real version instead of "10.x".

use std::fs;

fn main() {
	println!("cargo:rerun-if-changed=Cargo.lock");

	let lock = fs::read_to_string("Cargo.lock").expect("Cargo.lock not found");
	let version = find_crate_version(&lock, "buttplug_client")
		.expect("buttplug_client missing from Cargo.lock — did the dep get renamed?");

	println!("cargo:rustc-env=BUTTPLUG_VERSION={version}");
}

/// Scan the `[[package]]` blocks in a Cargo.lock string for the named crate
/// and return its resolved `version = "X.Y.Z"`. Avoids a build-time TOML
/// parser dependency for a single-field lookup.
fn find_crate_version(lock: &str, crate_name: &str) -> Option<String> {
	let name_line = format!("name = \"{crate_name}\"");
	for block in lock.split("[[package]]") {
		if !block.contains(&name_line) {
			continue;
		}
		for line in block.lines() {
			if let Some(rest) = line.strip_prefix("version = \"") {
				if let Some(end) = rest.find('"') {
					return Some(rest[..end].to_string());
				}
			}
		}
	}
	None
}

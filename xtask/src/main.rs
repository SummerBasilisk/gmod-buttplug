//! In-repo build helper. Runs `cargo build --release --target <triple>` for
//! gmod-buttplug and then copies the resulting cdylib to the GMod-expected
//! `gmcl_buttplug_<platform>.dll` filename alongside it. End users (and CI)
//! run `cargo xtask build [--target <triple>]` and get the ready-to-ship
//! artifact with no extra rename step.

use std::{env, path::PathBuf, process::Command};

fn main() {
	let mut args = env::args().skip(1);
	match args.next().as_deref() {
		Some("build") => {
			if let Err(e) = build(args.collect()) {
				eprintln!("xtask: {e}");
				std::process::exit(1);
			}
		}
		_ => {
			eprintln!("usage: cargo xtask build [--target <triple>] [extra cargo args]");
			std::process::exit(2);
		}
	}
}

fn build(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
	// Pull --target out of the arg list; pass the rest through to cargo.
	let (target, pass_through) = split_out_target(args);
	let target = target.unwrap_or_else(host_default_target);

	// Validate the target upfront so we don't do a 3-minute build only to
	// fail at the rename step.
	let (src_name, dst_name) = platform_names(&target)?;

	let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
	let status = Command::new(&cargo)
		.arg("build")
		.arg("--release")
		.arg("--package").arg("gmod-buttplug")
		.arg("--target").arg(&target)
		.args(&pass_through)
		.current_dir(workspace_root())
		.status()?;
	if !status.success() {
		std::process::exit(status.code().unwrap_or(1));
	}

	let release_dir = workspace_root().join("target").join(&target).join("release");
	let src_path = release_dir.join(src_name);
	let dst_path = release_dir.join(dst_name);
	std::fs::copy(&src_path, &dst_path)?;
	println!("xtask: wrote {}", dst_path.display());
	Ok(())
}

fn split_out_target(args: Vec<String>) -> (Option<String>, Vec<String>) {
	let mut target = None;
	let mut rest = Vec::with_capacity(args.len());
	let mut iter = args.into_iter();
	while let Some(arg) = iter.next() {
		if arg == "--target" {
			target = iter.next();
		} else if let Some(val) = arg.strip_prefix("--target=") {
			target = Some(val.to_string());
		} else {
			rest.push(arg);
		}
	}
	(target, rest)
}

fn workspace_root() -> PathBuf {
	// xtask's manifest dir is <root>/xtask, so the workspace root is its parent.
	PathBuf::from(env!("CARGO_MANIFEST_DIR"))
		.parent()
		.expect("xtask/Cargo.toml should be one level below the workspace root")
		.to_path_buf()
}

fn host_default_target() -> String {
	if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
		"x86_64-pc-windows-msvc".into()
	} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
		"x86_64-unknown-linux-gnu".into()
	} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
		"x86_64-apple-darwin".into()
	} else {
		eprintln!("xtask: no default target for this host; pass --target <triple>");
		std::process::exit(1);
	}
}

/// Map a rustc target triple to `(cargo output name, final GMod name)`.
fn platform_names(target: &str) -> Result<(&'static str, &'static str), String> {
	Ok(match target {
		"x86_64-pc-windows-msvc"    => ("gmcl_buttplug.dll",     "gmcl_buttplug_win64.dll"),
		"i686-pc-windows-msvc"      => ("gmcl_buttplug.dll",     "gmcl_buttplug_win32.dll"),
		"x86_64-unknown-linux-gnu"  => ("libgmcl_buttplug.so",   "gmcl_buttplug_linux64.dll"),
		"i686-unknown-linux-gnu"    => ("libgmcl_buttplug.so",   "gmcl_buttplug_linux.dll"),
		"x86_64-apple-darwin"       => ("libgmcl_buttplug.dylib", "gmcl_buttplug_osx64.dll"),
		other => return Err(format!("unsupported target: {other}")),
	})
}

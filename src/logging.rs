//! Runtime-togglable tracing subscriber.
//!
//! buttplug / btleplug / tokio all emit structured events via the `tracing`
//! crate. Without a subscriber installed those events are silently dropped,
//! which makes BLE-discovery issues almost impossible to diagnose from inside
//! a gmod session.
//!
//! We install a single global subscriber at module load time with an
//! [`EnvFilter`] wrapped in a [`reload::Layer`], and expose the reload handle
//! to Lua via `buttplug.SetLogFilter(spec)`. That lets a player flip on
//! verbose btleplug logging mid-session without restarting the game.
//!
//! Defaults to `warn` so release builds aren't noisy; `RUST_LOG` overrides
//! the initial filter if set before gmod launches.
//!
//! # Why we call `tier0!ConColorMsg` directly instead of `println!`
//!
//! gmod-rs's [`gmod::gmcl::override_stdout`] routes `println!` / `print!` to
//! the dev console via `std::io::set_output_capture` — which is
//! **thread-local**. Only whichever thread called `override_stdout()` gets
//! the hook installed. Tokio worker threads (where every buttplug / btleplug
//! tracing event fires) never do, so their `print!` calls hit real stdout,
//! which on a Windows GUI process is effectively `NUL`.
//!
//! Source Engine's dev console isn't fed from stdout anyway: tier0's spew
//! system is. Every native module (HolyLib et al.) just calls `Msg` /
//! `ConColorMsg`. gmod-rs helpfully pre-resolves `ConColorMsg` as a public
//! lazy_static (see gmod-17.0.0/src/msgc.rs), so we just call it directly
//! from the tracing writer — works from any thread, synchronous, no 250 ms
//! capture-poll latency.

use std::ffi::CString;
use std::io::{self, Write};
use std::sync::OnceLock;

use gmod::msgc::{Color, ConColorMsg};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter, prelude::*, reload};

// White, matching tier0's default spew color for `Msg`. Swap per-level later
// if we ever want warnings in red.
const DEFAULT_COLOR: Color = Color::new(200, 200, 200);

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

struct ConsoleWriter;

impl Write for ConsoleWriter {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		// CRITICAL: feed the payload as the `%s` argument, never as the
		// format string. Tracing output contains `%` characters (timestamps,
		// duration fields), and passing them as a format would be undefined
		// behavior — the varargs machinery would read uninitialized slots
		// off the stack.
		let cstr = match CString::new(String::from_utf8_lossy(buf).as_bytes()) {
			Ok(c)  => c,
			Err(_) => return Ok(buf.len()), // interior NUL — just drop
		};
		unsafe {
			ConColorMsg(&DEFAULT_COLOR, c"%s".as_ptr(), cstr.as_ptr());
		}
		Ok(buf.len())
	}
	fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl<'a> MakeWriter<'a> for ConsoleWriter {
	type Writer = ConsoleWriter;
	fn make_writer(&'a self) -> Self::Writer { ConsoleWriter }
}

// ---------------------------------------------------------------------------
// Subscriber
// ---------------------------------------------------------------------------

/// Type-erased "apply this filter" function. The concrete reload handle type
/// depends on the full layered-subscriber composition, so instead of spelling
/// that type out we close over the handle and hand back a plain function.
type ReloadFn = Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>;

static RELOAD: OnceLock<ReloadFn> = OnceLock::new();

/// Install the subscriber. Safe to call more than once — subsequent calls are
/// no-ops (the OnceLock already holds the reload handle, and `try_init`
/// refuses to overwrite a previously-set global subscriber).
pub(crate) fn init() {
	let initial = EnvFilter::try_from_default_env()
		.unwrap_or_else(|_| EnvFilter::new("warn"));

	let (filter_layer, handle) = reload::Layer::new(initial);

	let reload_fn: ReloadFn = Box::new(move |new_filter| {
		handle.reload(new_filter).map_err(|e| format!("{e}"))
	});
	let _ = RELOAD.set(reload_fn);

	let _ = tracing_subscriber::registry()
		.with(filter_layer)
		.with(
			tracing_subscriber::fmt::layer()
				.with_ansi(false)
				.with_writer(ConsoleWriter)
		)
		.try_init();
}

/// Parse `spec` (e.g. `"debug"`, `"btleplug=trace,buttplug=debug"`) and apply
/// it to the running subscriber. Returns `Err` if the spec is malformed or
/// the subscriber wasn't initialized.
pub(crate) fn set_filter(spec: &str) -> Result<(), String> {
	let filter: EnvFilter = spec.parse().map_err(|e| format!("invalid filter: {e}"))?;
	match RELOAD.get() {
		Some(f) => f(filter),
		None    => Err("log subsystem not initialized".into()),
	}
}

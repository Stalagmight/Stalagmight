//! Harness-only debug hooks. Two layers keep these inert for real users:
//! every command is a no-op unless `STALAGMIGHT_TEST_HOOKS=1` is set in the
//! process environment (the agent test harness sets it; normal launches never
//! do), and the deliberately-crashing body is additionally **compiled out** of
//! release builds - a production binary contains no code path that can panic
//! here even if the env var leaks. The command itself stays registered in all
//! builds so the invoke-handler/telemetry-registry contract is build-invariant.

use crate::commands::pool;
use crate::core::CommandError;

/// Trigger a deliberate backend panic on a background thread so the agent
/// harness can self-test its crash oracle ("harness doctor"). The panic is
/// isolated to the spawned thread, so the main app keeps running (runtime is
/// unwind) while the panic hook fires and writes a crash marker. Returns `Err`
/// (a no-op) unless `STALAGMIGHT_TEST_HOOKS=1`; in a release build without the
/// `e2e` feature it returns `Err` unconditionally (the panic is compiled out).
#[tauri::command]
pub async fn debug_trigger_panic(delay_ms: Option<u64>) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        if std::env::var("STALAGMIGHT_TEST_HOOKS").as_deref() != Ok("1") {
            return Err(CommandError::failed(
                "test hooks disabled (set STALAGMIGHT_TEST_HOOKS=1)",
            ));
        }
        #[cfg(any(debug_assertions, feature = "e2e"))]
        {
            let delay = delay_ms.unwrap_or(50);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(delay));
                trigger_test_panic();
            });
            Ok(())
        }
        #[cfg(not(any(debug_assertions, feature = "e2e")))]
        {
            let _ = delay_ms;
            Err(CommandError::failed(
                "debug_trigger_panic is compiled out of release builds",
            ))
        }
    })
    .await
}

/// The deliberate crash, isolated into its own function so the per-site
/// `#[expect(clippy::panic)]` stays tightly scoped. `#[expect]` (not `#[allow]`,
/// which the crate denies) is the sanctioned way to permit a lint at a single
/// justified site; see docs/linting.md.
#[cfg(any(debug_assertions, feature = "e2e"))]
#[expect(clippy::panic)]
fn trigger_test_panic() -> ! {
    panic!("deliberate harness self-test panic");
}

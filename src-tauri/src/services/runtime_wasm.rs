// spec-kit 001 · T028 — WASM runtime for untrusted-logic plugins.
//
// Isolation by CONSTRUCTION: a module is instantiated with an EMPTY linker — no
// host functions are provided, so the guest has NO way to reach the network, the
// filesystem, or any syscall. This is the strongest sandbox in the plugin host
// (stronger than the subprocess+OS-sandbox path) and is the recommended runtime
// for third-party / untrusted plugins (council rule). Fuel-metered to bound CPU.
//
// Plugin ABI (v1, linear-memory string passing):
//   - export `memory`
//   - export `alloc(len: i32) -> i32`            (guest allocator → ptr)
//   - export `furx_run(in_ptr: i32, in_len: i32) -> i64`
//        return value packs (out_ptr: u32 high, out_len: u32 low)
// The host writes the args JSON into guest memory via `alloc`, calls `furx_run`,
// and reads the result bytes back. No imports → the guest cannot do I/O.
//
// Off by default (YAGNI); compile with `--features wasm-runtime`.
#![cfg(feature = "wasm-runtime")]

use anyhow::{anyhow, Result};
use wasmtime::{Caller, Engine, Linker, Module, Store};

/// Per-call CPU bound (fuel units). ~tens of millions of ops; tune as needed.
const FUEL: u64 = 200_000_000;

struct HostState; // intentionally empty — no capabilities exposed to the guest.

/// Run a WASM plugin tool. `wasm` is the module bytes (.wasm or .wat). `args_json`
/// is passed to the guest's `furx_run`; the returned bytes are the tool output.
/// The guest has zero host imports → no network, no filesystem, no syscalls.
pub fn run_wasm_tool(wasm: &[u8], args_json: &str) -> Result<String> {
    let mut config = wasmtime::Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, wasm)?;
    let mut store = Store::new(&engine, HostState);
    store.set_fuel(FUEL)?;

    // Empty linker: NO host functions. The guest is fully sandboxed.
    let linker: Linker<HostState> = Linker::new(&engine);
    let instance = linker.instantiate(&mut store, &module)?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| anyhow!("plugin missing `memory` export"))?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .map_err(|_| anyhow!("plugin missing `alloc` export"))?;
    let run = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "furx_run")
        .map_err(|_| anyhow!("plugin missing `furx_run` export"))?;

    // Write args into guest memory.
    let bytes = args_json.as_bytes();
    let in_ptr = alloc.call(&mut store, bytes.len() as i32)?;
    memory.write(&mut store, in_ptr as usize, bytes)?;

    // Call the tool.
    let packed = run.call(&mut store, (in_ptr, bytes.len() as i32))?;
    let out_ptr = (packed >> 32) as u32 as usize;
    let out_len = (packed & 0xffff_ffff) as u32 as usize;

    let mut out = vec![0u8; out_len];
    memory.read(&store, out_ptr, &mut out)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

// Silence unused warning for the empty host-state field pattern.
#[allow(dead_code)]
fn _assert_no_host_imports(_c: Caller<'_, HostState>) {}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny WAT plugin implementing the ABI: alloc returns a scratch ptr; furx_run
    // writes a fixed result ("OK") via a data segment and returns its (ptr<<32)|len.
    // Proves the full ABI round-trip (alloc + call + guest memory read by host) and
    // that a module with NO imports runs fully sandboxed (no net/fs possible).
    const ABI_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "alloc") (param $len i32) (result i32) (i32.const 2000))
          (func (export "furx_run") (param $in_ptr i32) (param $in_len i32) (result i64)
            (i32.store8 (i32.const 100) (i32.const 79))  ;; 'O'
            (i32.store8 (i32.const 101) (i32.const 75))  ;; 'K'
            (i64.or
              (i64.shl (i64.const 100) (i64.const 32))
              (i64.const 2))))
    "#;

    #[test]
    fn wasm_plugin_abi_roundtrip_sandboxed() {
        let out = run_wasm_tool(ABI_WAT.as_bytes(), "{\"hello\":\"world\"}").unwrap();
        assert_eq!(out, "OK");
    }

    #[test]
    fn module_requiring_host_import_is_rejected() {
        // A module that imports a host function can't instantiate against the empty
        // linker → no way to smuggle in network/fs access.
        let wat = r#"(module (import "env" "evil" (func)) (memory (export "memory") 1))"#;
        assert!(run_wasm_tool(wat.as_bytes(), "{}").is_err());
    }
}

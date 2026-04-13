
## Implementation: /rescan and /memory inline commands

### Key pattern discovered
- `handle_inline_command()` is sync, but `SystemInfo::detect()` is async
- Solution: Use Result variant pattern - return `InlineCommandResult::Rescan` from sync handler, then handle the async work in `run_tui()` loop
- This avoids making `handle_inline_command()` async and keeps the REPL responsive

### Changes made
1. `src/cli/commands.rs`: Changed `fn memory_md_path()` to `pub(crate) fn memory_md_path()` to allow reuse in tui module
2. `src/tui/mod.rs`:
   - Added `Rescan` variant to `InlineCommandResult` enum
   - Added match arms for "rescan"/"/rescan" and "memory"/"/memory" in `handle_inline_command()`
   - Added `rescan_system_info()` async helper that calls `crate::system_info::detect().await`
   - Updated `run_tui()` to handle `InlineCommandResult::Rescan` with proper async flow
   - Updated `print_help()` and `print_welcome()` to include new commands
   - Added tests `rescan_is_recognized` and `memory_command_is_recognized`

### Gotcha
- `detect()` is a standalone async function, NOT an associated method on `SystemInfo`
- Correct: `crate::system_info::detect().await`
- Wrong: `crate::system_info::SystemInfo::detect().await`

### Test verification
- All 244 tests pass
- Build succeeds

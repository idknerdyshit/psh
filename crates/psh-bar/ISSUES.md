# psh-bar — Known Issues

Remaining issues from code review (2026-03-21). Listed roughly by impact.

## Moderate

### 1. `try_send` silently drops messages on full channels

All async modules use `try_send` on bounded channels (capacity 4–16). If events arrive faster than GTK processes them, messages are silently dropped and the UI shows stale state. Backend tasks should use `.send().await` where they can afford to wait, or channel capacity should be increased.

**Files:** all modules, `main.rs` (IPC fan-out at line 181)

### 2. Disconnected IPC client write halves accumulate until broadcast

When a client disconnects, its read task exits but the `OwnedWriteHalf` stays in the `clients` vec until the next `broadcast()` call detects the write failure. If no outbound messages are flowing, dead clients leak indefinitely. The read task should signal removal of the corresponding write half.

**File:** `src/main.rs`

### 3. `show_all_workspaces` config field is ignored

`BarConfig.show_all_workspaces` exists but `rebuild_workspace_buttons` always displays all workspaces from all outputs. When `false`, it should filter to the focused output only.

**File:** `src/modules/workspaces.rs`

### 4. Network ethernet display discards connection name

`format_network_state` returns just `"ETH"` for ethernet, ignoring `state.name`. On systems with multiple ethernet connections, they are indistinguishable. Consider showing the interface name or adding it as a tooltip.

**File:** `src/modules/network.rs:215`

### 5. `glib::DateTime::now_local().unwrap()` can panic

`now_local()` returns `Option<DateTime>` and can return `None` with a misconfigured timezone. Since the clock ticks every second, a panic kills the entire bar. Should use `unwrap_or_else` with a fallback or skip the update.

**File:** `src/modules/clock.rs:34`

## Low Priority

### 6. Network module polls every 5s instead of using D-Bus signals

The module polls NetworkManager state every 5 seconds. Using NM's `PropertiesChanged` D-Bus signal would give instant updates and eliminate unnecessary wake-ups.

**File:** `src/modules/network.rs:112-118`

### 7. No wifi signal strength

`get_wifi_signal()` returns `None` unconditionally. Implementing this requires traversing the NM device tree to the active access point's `Strength` property.

**File:** `src/modules/network.rs:203-209`

### 8. CSS missing `.psh-bar-workspace-btn.focused:hover` state

There is a `.focused` style but no `.focused:hover` — the generic `:hover` style applies, which may look inconsistent since `focused` has `font-weight: bold` but hover doesn't.

**File:** `assets/themes/default.css`

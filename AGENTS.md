# AgentStack — Complete Agent Framework

## Project Info

- **Location**: `C:/Users/reven/AgentStack/`
- **Language**: Rust (edition 2021, MSRV 1.75)
- **Toolchain**: stable-x86_64-pc-windows-msvc (cargo 1.98)
- **Dependencies (required)**: serde, serde_json, uuid, parking_lot, chrono, clap, thiserror
- **Dependencies (optional, feature `llm`)**: reqwest, tokio
- **Dev dependencies**: tempfile
- **Test framework**: `cargo test` (built-in)
- **Tests**: 74 unit tests + 2 integration tests (`tests/test_integration.rs`), all passing
- **Benchmarks**: `benches/framework.rs` (criterion) — `cargo bench`
- **Version**: 0.1.0

## What this is

A standalone, working agent orchestration framework that implements the "graph +
loops" baseline PLUS the 13 layers most agent frameworks are missing:
coordination substrate, causal memory, trust propagation, goal/intent model,
meta-cognition, resource budget, interrupt propagation, negotiation/contracting,
determinism/replay, input validation, memory decay, and human-in-the-loop.

It is **separate from VerifyStack** (which is a static code-verification engine).
Useful primitives from VerifyStack (per-file cache pattern, graph dataclass
style, check-base interface, sandbox) were borrowed conceptually; VerifyStack
itself was NOT modified.

## Commands

```bash
# Build
cargo build
cargo build --release
cargo build --features llm       # enable the real LLM agent

# Test
cargo test                      # all unit + integration tests
cargo test --test test_integration

# Run examples
cargo run --example hello_graph
cargo run --example full_stack

# CLI
cargo run -- run examples/spec.json
cargo run -- run examples/spec.json --trace trace.json
cargo run -- layers
```

## Architecture — 13 layers

| # | Layer | Module | Status |
|---|---|---|---|
| 1 | Graph + loops | `graph.rs`, `loop_spec.rs`, `executor.rs` | done, tested |
| 2 | Coordination substrate | `blackboard.rs`, `policy.rs` | done, tested |
| 3 | Causal memory | `causal.rs`, `trace_store.rs` | done, tested |
| 4 | Trust propagation | `trust.rs` | done, tested |
| 5 | Goal/intent model | `goal.rs` | done, tested |
| 6 | Meta-cognition | `meta.rs` | done, tested |
| 7 | Resource budget | `budget.rs` | done, tested |
| 8 | Interrupt propagation | `interrupt.rs`, `error.rs` | done, tested |
| 9 | Negotiation/contracting | `contract.rs`, `negotiation.rs` | done, tested |
| 10 | Determinism/replay | `record.rs`, `replay.rs` | done, tested |
| 11 | Input validation | `validate.rs` | done, tested |
| 12 | Memory decay | `memory.rs` | done, tested |
| 13 | Human-in-the-loop | `human.rs` | done, tested |

## Core abstractions

- `Agent` trait (`agent.rs`) — the unit of work. Implement this for your agents.
- `AgentContext` — per-invocation view exposing every layer (blackboard, trust,
  budget, goal, meta channel, interrupt bus, run record).
- `Runtime` (`runtime.rs`) — owns all shared services + agent registry. `run()`
  seeds the blackboard and drives a `GraphExecutor`.
- `GraphExecutor` (`executor.rs`) — the integration point. On each node it:
  checks interrupts (8) + budget (7), opens a causal span (3), validates input
  (11), builds the context with inherited trust (4), runs the agent, checks goal
  alignment (5), collects meta signals (6), writes output to the blackboard (2),
  records model calls (10), handles loops (1), and follows edges (incl. OnError).

## Key design decisions

- **No required network/LLM.** Default build runs deterministically with mock
  agents. Real LLM agent behind `llm` feature.
- **`Arc<dyn Agent>`** so one agent can be reused across loop iterations and
  multiple nodes without remove-and-reinsert.
- **`Value` equality is order-independent for objects** (compared as a map) so
  JSON round-trips through `serde_json::Map` don't break equality.
- **Trust narrowing uses prefix grants**: parent `filesystem` grants child
  `filesystem:write` (mirrors capability semantics).
- **Blackboard `update`** holds the slots write lock for the whole RMW (executor
  is sequential today; contract stays correct under future concurrency).
- **Determinism**: agents record every model call via `ctx.record_model_call`;
  `Replayer` serves recorded responses by request key.
- **Subgraph recursion**: child graphs share all services (blackboard, budget,
  trace, goal, interrupts, record) so causal memory is continuous across the
  composition boundary. Trust narrows automatically. Step budget is shared
  globally (parent + child).
- **Human timeout**: `std::thread` + `mpsc::channel` + `recv_timeout` — keeps
  the framework fully synchronous while giving a hard timeout guarantee. On
  timeout, a `HumanRejected` interrupt is raised and the run escalates.
- **LLM token parsing**: the `llm` agent parses `usage{total_tokens}` from the
  API response and spends from the budget (layer 7 integration).

## Known limitations / future work

- The LLM agent uses `reqwest::blocking` (sync HTTP). An async variant with
  streaming responses would require tokio but isn't needed for the deterministic
  core.
- The benchmark harness (`benches/framework.rs`) measures framework overhead
  with mock agents; real-world benchmarks would need LLM call mocking.
- Subgraph nodes share all services (blackboard, budget, trace, etc.) with the
  parent; an isolation mode (separate blackboard namespace) could be added.

## Verification

- `cargo build` — clean, no warnings.
- `cargo test` — 74 unit + 2 integration, all green.
- `cargo build --release` — clean.
- `cargo build --features llm` — compiles (LLM agent with real token parsing).
- `cargo bench --no-run` — benchmark harness compiles (criterion).
- `cargo run --example full_stack` — exercises all 13 layers, asserts count==5.
- `cargo run -- run examples/spec.json` — CLI runs a JSON spec.
- Integration test verifies: subgraph recursion (doubles count 3→6), human
  timeout escalation (HumanRejected interrupt), deterministic replay.

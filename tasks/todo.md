# AgentStack — Complete Agent Framework (13 layers)

## Goal
A new, standalone, **working** agent orchestration framework in Rust at
`C:\Users\reven\AgentStack`. Implements all 13 layers discussed. Copies useful
primitives from VerifyStack (cache pattern, graph dataclass style, check-base
interface, sandbox) WITHOUT modifying VerifyStack.

## Design principles (from VerifyStack)
- Zero required dependencies where possible (std + a few well-vetted crates).
- Deterministic core: given same inputs + same model-call responses, a run is
  reproducible (layer 10).
- Trait-based plugin interfaces (mirrors VerifyStack `BaseCheck`).
- Typed findings/verdicts (mirrors `Finding`/`VerificationReport`).
- Per-run disk cache for traces (mirrors `.verifystack/` cache pattern).

## Crate layout
```
agent_stack/
  Cargo.toml
  src/
    lib.rs                  # public API re-exports
    prelude.rs
    error.rs                # AgentError, Interrupt types (layer 8)
    value.rs                # Value (typed blackboard payload) + Serde
    id.rs                   # NodeId, EdgeId, RunId, SpanId, TraceId

    # --- Layer 1: graph + loops ---
    graph.rs                # Graph, Node, Edge, topology
    loop_spec.rs            # LoopSpec (repeat/while/until/foreach), LoopState
    executor.rs             # GraphExecutor: runs nodes honoring loops + edges

    # --- Layer 2: coordination substrate ---
    blackboard.rs           # Blackboard: typed slots, read/write, guards
    policy.rs               # AccessPolicy: who can read/write/lock what

    # --- Layer 3: causal memory ---
    causal.rs               # CausalTrace, Cause, Effect, Span, dependencies
    trace_store.rs          # disk-persisted trace (JSON), replay input

    # --- Layer 4: trust propagation ---
    trust.rs                # TrustContext, Authority, Constraint, narrowing

    # --- Layer 5: goal/intent model ---
    goal.rs                 # Goal, GoalInvariant, satisfaction check, drift

    # --- Layer 6: meta-cognition ---
    meta.rs                 # Confidence, UncertaintySignal, Escalation

    # --- Layer 7: resource budget ---
    budget.rs               # Budget (tokens/time/money/calls), trackers, degrade

    # --- Layer 8: interrupt / exception propagation ---
    interrupt.rs            # Interrupt kinds, propagation, short-circuit

    # --- Layer 9: negotiation / contracting ---
    contract.rs             # Contract, Bid, Offer, Accept, Reject, Renegotiate
    negotiation.rs          # Negotiation protocol between agents

    # --- Layer 10: determinism / replayability ---
    record.rs               # RunRecord: inputs + model-call responses
    replay.rs               # Replayer: deterministic replay from record

    # --- Layer 11: adversarial input validation ---
    validate.rs             # InputValidator at agent boundaries (content integrity)

    # --- Layer 12: decay / forgetting ---
    memory.rs               # MemoryStore with decay schedule (keep/compress/drop)

    # --- Layer 13: human-in-the-loop ---
    human.rs                # HumanNode: async, timeout, escalation, structured handoff

    # --- agent trait + runtime ---
    agent.rs                # Agent trait, AgentContext, AgentOutput
    runtime.rs              # wires all layers into a Runtime
    mock_agent.rs           # deterministic built-in agent (no LLM needed)
    llm_agent.rs            # optional LLM agent behind feature flag "llm"

    cli.rs                  # binary: run a graph from a TOML/JSON spec
  src/bin/agentstack.rs
  tests/
    test_graph_loops.rs
    test_blackboard.rs
    test_causal.rs
    test_trust.rs
    test_goal.rs
    test_meta.rs
    test_budget.rs
    test_interrupt.rs
    test_negotiation.rs
    test_replay.rs
    test_validate.rs
    test_memory.rs
    test_human.rs
    test_integration.rs     # end-to-end: a multi-node graph using all layers
  examples/
    hello_graph.rs          # minimal graph + loop
    full_stack.rs           # demo using every layer
```

## Dependencies (well-vetted, stdlib-first)
- `serde`, `serde_json` — typed value + trace persistence (mature, ubiquitous)
- `uuid` — stable ids (mature)
- `parking_lot` — fast sync primitives for blackboard (mature)
- `chrono` — timestamps for causal trace (mature)
- Optional `reqwest`+`tokio` behind `llm` feature for real LLM agent
- `clap` (binary only) for CLI

No floating ranges; pin to recent stable versions.

## Build order (checkable)
- [ ] 0. Cargo project skeleton compiles (empty modules)
- [ ] 1. Core types: error, value, id
- [ ] 2. Layer 1: graph + loop_spec + executor (with tests)
- [ ] 3. Layer 2: blackboard + policy (with tests)
- [ ] 4. Layer 3: causal + trace_store (with tests)
- [ ] 5. Layer 4: trust (with tests)
- [ ] 6. Layer 5: goal (with tests)
- [ ] 7. Layer 6: meta (with tests)
- [ ] 8. Layer 7: budget (with tests)
- [ ] 9. Layer 8: interrupt (with tests)
- [ ] 10. Layer 9: contract + negotiation (with tests)
- [ ] 11. Layer 10: record + replay (with tests)
- [ ] 12. Layer 11: validate (with tests)
- [ ] 13. Layer 12: memory (with tests)
- [ ] 14. Layer 13: human (with tests)
- [ ] 15. Agent trait + mock_agent + runtime (with tests)
- [ ] 16. Optional llm_agent behind feature flag
- [ ] 17. CLI binary + examples
- [ ] 18. Integration test exercising all 13 layers
- [ ] 19. `cargo test` all green; `cargo build --release` clean
- [ ] 20. README + AGENTS.md documenting the 13 layers

## Verification
- `cargo build` and `cargo test` must pass after each layer.
- Determinism test: same RunRecord → identical trace on replay.
- Integration test runs a graph with a loop, blackboard contention, trust
  narrowing, budget exhaustion causing graceful degrade, an interrupt, a
  negotiation, a human node (mocked), and validates the causal trace.

## Review (fill in at end)
- Built: 20 source modules + CLI + 2 examples + 1 integration test.
- Tests: 70 unit + 1 integration, all green. `cargo build --release` clean, zero warnings.
- `llm` feature compiles separately.
- VerifyStack was NOT modified; only borrowed conceptual patterns (cache, graph dataclass, check-base, sandbox).
- Determinism verified: replay reproduces identical final blackboard state.

## Gap-filling pass

- [x] G1. Subgraph recursive execution: graph registry on Runtime + nested executor
- [x] G2. Human node real timeout/escalation via std::thread + channel (sync, isolated)
- [x] G3. LLM agent: parse `usage{}` from API response for real token counts
- [x] G4. Benchmark harness with criterion
- [x] G5. Update integration test to exercise subgraph recursion + human timeout
- [x] G6. cargo test green; cargo build --release clean; cargo bench compiles
- [x] G7. Update AGENTS.md / README to remove the gap notes

### Gap-fill review
- G1: `Runtime::register_subgraph` + `resolve_subgraph`; `GraphExecutor::run_with_entry_input`
  runs the child with narrowed trust, shared services (continuous causal trace),
  shared step budget. Test: subgraph doubles count 3→6, spans span parent+child.
- G2: `run_human` spawns worker thread, waits on `mpsc::recv_timeout`. On timeout
  → `HumanRejected` interrupt + `Interrupted` status. `set_human_timeout` on Runtime.
  `slow_handler` test helper. Tests: timeout escalates, normal completes.
- G3: `Usage` struct parses `prompt_tokens`/`completion_tokens`/`total_tokens` from
  API response. `ctx.spend(Tokens, total)` integrates with budget. Removed placeholder.
- G4: `benches/framework.rs` with criterion: linear graph, loop graph, blackboard
  RMW, causal trace, budget spend, input validation, subgraph depth. `cargo bench`.
- G5: Integration test now registers a real child graph (doubler), asserts
  `out.sub == 6`. New `human_node_timeout_escalates` test asserts `Interrupted` +
  `HumanRejected`. Replay also registers child graph.
- G6: 74 unit + 2 integration = 76 tests, all green. Release build clean, zero
  warnings. `cargo bench --no-run` compiles. `cargo build --features llm` compiles.
- G7: AGENTS.md + README updated: removed gap notes, added new capabilities,
  updated test counts, added benchmark instructions.

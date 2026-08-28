# AgentStack

> A complete agent orchestration framework in Rust. Graphs and loops — plus the
> 13 layers everyone else is missing.

Most agent frameworks ship **graphs + loops** (topology + iteration). That gets
you dynamic behavior. It does **not** get you controllable, explainable, safe
behavior at scale. AgentStack implements the full picture:

| # | Layer | Module(s) | What it gives you |
|---|---|---|---|
| 1 | Graph + loops | `graph`, `loop_spec`, `executor` | topology + iteration |
| 2 | Coordination substrate | `blackboard`, `policy` | shared state with arbitration |
| 3 | Causal memory | `causal`, `trace_store` | structured trace + dependencies |
| 4 | Trust propagation | `trust` | authority narrows through call chains |
| 5 | Goal / intent model | `goal` | an invariant that travels with the run |
| 6 | Meta-cognition | `meta` | confidence / escalation signals |
| 7 | Resource budget | `budget` | tokens / time / money, graceful degrade |
| 8 | Interrupt propagation | `interrupt`, `error` | structured, first-class failure |
| 9 | Negotiation / contracting | `contract`, `negotiation` | bid / offer / accept / reject |
| 10 | Determinism / replay | `record`, `replay` | record model calls, replay exactly |
| 11 | Input validation | `validate` | content integrity at agent boundaries |
| 11 | Memory decay | `memory` | keep / compress / drop working memory |
| 13 | Human-in-the-loop | `human` | a first-class node, not a bolt-on |

(Layers 1–2 give you dynamic behavior. Layers 3–13 give you controllable,
explainable, safe behavior — the pieces serious teams add only after an
incident.)

## Quick start

```bash
cargo build
cargo test                       # 74 unit tests + 2 integration tests
cargo run --example hello_graph  # minimal two-node graph + loop
cargo run --example full_stack   # exercises every layer, prints a report
cargo run -- run examples/spec.json   # run a graph from a JSON spec
cargo run -- layers              # print the 13-layer map
cargo bench                      # benchmark framework overhead (criterion)
```

No LLM, no network, no API key required. The framework runs deterministically
with mock agents. Enable the `llm` feature for a real OpenAI-compatible agent
(see `llm_agent.rs`).

## The 13 layers in one screen

```rust
use agent_stack::prelude::*;

// 1. Build a graph with a loop.
let mut g = Graph::new("demo");
let n = g.add_node(
    Node::agent("incr", "incr")
        .with_output_slot("out.incr")
        .with_loop(LoopSpec::repeat(3)),
);

// 2–13. Wire up a runtime with all the layers.
let rt = Runtime::new(
    Blackboard::permissive(),          // 2: coordination substrate
    Arc::new(Budget::unlimited()),     // 7: resource budget
    Arc::new(CausalTrace::new()),      // 3: causal memory
    Goal::trivial(),                   // 5: goal/intent
    Arc::new(InputValidator::new()),   // 11: input validation
);
rt.set_human_handler(human::auto_approve_handler()); // 13: human-in-the-loop

// Run it. The executor integrates every layer on each node.
let out = rt.run(&g, RunConfig::default().with_seed(vec![("count".into(), Value::int(0))]))?;
```

## Why each layer exists

- **Coordination substrate** — graphs pass messages on edges; loops hold local
  state. Neither gives agents a place to *coordinate through* with visibility,
  arbitration, and policy. The `Blackboard` is that place.
- **Causal memory** — loops are stateless between invocations; graphs don't
  model *why* a node fired. The `CausalTrace` records what triggered what, what
  state was read, what decision was made, what changed — so you can reflect,
  roll back, and verify trust.
- **Trust propagation** — when A→B→C, the permission context collapses in most
  frameworks. `TrustContext` *narrows* down the call chain: a child can never
  hold more authority than its parent, and constraints accumulate.
- **Goal/intent** — by node 4 the original goal is diluted through paraphrase.
  A `Goal` travels with the run and every output is checked against it
  (Goodhart's Law, applied at the agent level).
- **Meta-cognition** — agents emit `MetaSignal`s (confidence, "wrong agent",
  escalation, stop). The graph routes on them instead of running confidently
  into garbage.
- **Budget** — time/tokens/money/API calls have no native model in most graphs.
  `Budget` lets the runtime degrade gracefully instead of crashing at the limit.
- **Interrupts** — graphs have happy-path edges; loops have break conditions.
  Neither has a model for anomalous conditions that short-circuit the flow with
  structured context. `InterruptBus` is that model.
- **Negotiation** — "do this" isn't cooperation. `Contract` + `Negotiation` give
  agents a bid/offer/accept/reject/renegotiate protocol.
- **Replay** — most agent systems are non-deterministic *by accident*. Every
  model call is recorded into a `RunRecord`; `Replayer` feeds them back so a
  run is reproducible — every bug becomes reproducible.
- **Input validation** — agents receive inputs from other agents that can be
  wrong, poisoned (prompt injection), or malformed. `InputValidator` checks
  content integrity at boundaries (trust handles *authority*; this handles
  *content*).
- **Memory decay** — memory that never forgets becomes noise. `MemoryStore`
  keeps, compresses, or drops entries on a schedule so agents don't reason
  against ghosts of old runs.
- **Human-in-the-loop** — review is a node type with the same contract as any
  other: it produces a `Value`, can time out, can escalate, hands back into the
  graph. Not special-cased.

## Project layout

```
src/
  lib.rs            public API + module declarations
  prelude.rs        common imports
  error.rs          AgentError + Interrupt (layer 8)
  value.rs          typed blackboard payload
  id.rs             stable newtype ids
  graph.rs          Graph / Node / Edge            (layer 1)
  loop_spec.rs      LoopSpec / LoopState           (layer 1)
  executor.rs       GraphExecutor — integrates all layers
  blackboard.rs     Blackboard                     (layer 2)
  policy.rs         AccessPolicy                   (layer 2)
  causal.rs         CausalTrace / Span / Effect    (layer 3)
  trace_store.rs    disk-persisted trace           (layer 3)
  trust.rs          TrustContext / Authority       (layer 4)
  goal.rs           Goal / GoalReport              (layer 5)
  meta.rs           Confidence / MetaSignal        (layer 6)
  budget.rs         Budget                         (layer 7)
  interrupt.rs      InterruptBus                   (layer 8)
  contract.rs       Contract                       (layer 9)
  negotiation.rs    Negotiation                    (layer 9)
  record.rs         RunRecord                      (layer 10)
  replay.rs         Replayer                       (layer 10)
  validate.rs       InputValidator                 (layer 11)
  memory.rs         MemoryStore                    (layer 12)
  human.rs          human handlers                 (layer 13)
  agent.rs          Agent trait + AgentContext
  mock_agent.rs     deterministic built-in agents
  runtime.rs        Runtime — wires all layers
  llm_agent.rs      optional real LLM agent (feature "llm")
  bin/agentstack.rs CLI
tests/test_integration.rs   end-to-end, all 13 layers + subgraph + human timeout
examples/                   hello_graph, full_stack, spec.json
benches/framework.rs        criterion benchmarks (linear, loop, blackboard, causal, budget, validation, subgraph)
```

## Design principles

- **Zero required network deps.** The default build is pure Rust + a few
  well-vetted crates (`serde`, `uuid`, `parking_lot`, `chrono`, `clap`,
  `thiserror`). The real LLM agent is behind the `llm` feature.
- **Deterministic core.** Given the same inputs and the same recorded
  model-call responses, a run is reproducible (layer 10).
- **Trait-based plugins.** Implement `Agent` for your own agents; the framework
  ships `MockAgent`, `BlackboardReader`, `BlackboardWriter`, and (optional)
  `LlmAgent`.
- **Typed everything.** `Value` is a typed payload; ids are newtypes so you
  can't mix a `NodeId` with an `EdgeId` at compile time.
- **Hierarchical composition.** Register a subgraph via
  `rt.register_subgraph(graph)` and reference it with `NodeKind::Subgraph(id)`.
  The child shares all services (blackboard, budget, trace, goal, interrupts,
  record) so causal memory is continuous across the composition boundary. Trust
  narrows automatically; step budget is shared globally.
- **Hard human timeout.** Human nodes run the handler in a worker thread and
  wait with `recv_timeout`. On timeout, a `HumanRejected` interrupt escalates
  the run — no async runtime needed.
- **Real token accounting.** The `llm` agent parses `usage{total_tokens}` from
  the API response and spends from the budget (layer 7 integration).

## License

MIT OR Apache-2.0.

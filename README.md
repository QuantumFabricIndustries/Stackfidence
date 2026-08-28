# Stackfidence

**The 8-layer agent execution substrate for Rust.**

Build AI agent systems you can actually trust â€” with typed interrupts, causal tracing, deterministic replay, budget enforcement, and trust propagation built in from day one.

[![Crates.io](https://img.shields.io/crates/v/stackfidence.svg)](https://crates.io/crates/stackfidence)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

---

## Why Stackfidence?

Every agent framework gives you graphs and loops. That gets you topology and iteration. It doesn't get you a system you can trust in production.

Stackfidence closes the gaps:

| Layer | What it gives you |
|---|---|
| Graph + loops | Topology + iteration |
| Coordination substrate | Shared blackboard with arbitration |
| Causal memory | Explainability + deterministic replay |
| Trust propagation | Safety through call chains |
| Goal/intent model | Alignment from start to finish |
| Meta-cognition | Knowing when you're wrong |
| Resource budget | Graceful degradation |
| Interrupt model | Structured failure handling |

---

## Features

- **76 passing tests** â€” 74 unit + 2 integration, zero failures
- **Deterministic replay** â€” identical blackboard state on re-run, every time
- **Hard timeouts on human nodes** â€” no graph held hostage by a missing human response
- **Typed interrupts** â€” `HumanRejected`, `BudgetExceeded`, and more â€” routable, not swallowed
- **Subgraph recursion** with continuous causal trace across recursion boundaries
- **8 benchmark groups** â€” linear graph, loop graph, blackboard contention, causal trace, budget, and more
- **Zero warnings** across debug, release, and LLM feature builds

---

## Quick Start

```toml
[dependencies]
stackfidence = "0.1.0"
```

---

## Architecture

Stackfidence is built on 20 source modules implementing all 13 internal layers:

- **Blackboard** â€” shared coordination substrate with contention handling
- **Causal trace** â€” full dependency chain for every node execution
- **Budget engine** â€” token, time, and cost-aware execution
- **Interrupt propagation** â€” structured failure as a first-class citizen
- **Trust layer** â€” policy enforcement through agent call chains
- **Human nodes** â€” with hard worker-thread timeouts
- **LLM agent** â€” real token usage parsed from the API response
- **Subgraph executor** â€” recursive with continuous causal trace

---

## Benchmarks

```
cargo bench
```

8 benchmark groups: linear graph, loop graph, blackboard contention,
blackboard graph, causal trace, budget, input validation, subgraph depth.

---

## Licensing

Stackfidence is dual-licensed:

- **Free** for individuals and open source projects (MIT / Apache 2.0)
- **Commercial license** required for use in proprietary/production systems

For commercial licensing: open an issue or contact via GitHub.

---

## Status

Active development. Core substrate is stable and fully tested.
PHANTOM/SID cognitive architecture integration in progress.

---

*Built by [Quantum Fabric Industries](https://github.com/pleggtheking)*

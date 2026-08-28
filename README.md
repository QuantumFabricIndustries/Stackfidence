# AgentStack

**The Rust agent execution substrate. Typed interrupts, causal tracing, deterministic replay, and trust propagation — built in.**

AgentStack is the execution layer for AI agent systems that need to be auditable, recoverable, and safe by construction. It gives you a principled foundation for running agents that you can actually reason about after the fact.

---

## The Problem

Most agent runtimes are held together with async callbacks, shared mutable state, and optimism. When something goes wrong — and it will — you have no execution history, no causal chain, no way to reproduce the failure, and no trust boundaries between components.

AgentStack is built for the opposite assumption: agents will fail, need to be interrupted, need to be audited, and need to operate with varying levels of trust across their component graph.

---

## Core Primitives

### Typed Interrupts
Every interruption point in an agent's execution is typed. You know *why* execution stopped — policy violation, resource limit, external signal, trust boundary — not just *that* it stopped.

```rust
match agent.run().await {
    Ok(result) => handle_result(result),
    Err(Interrupt::PolicyViolation(policy, ctx)) => audit_log(policy, ctx),
    Err(Interrupt::TrustBoundary(from, to, action)) => escalate(from, to, action),
    Err(Interrupt::ResourceLimit(kind, limit)) => backoff(kind, limit),
}
```

### Causal Tracing
Every action an agent takes is stamped with a causal ID linking it to the decision that produced it. You can trace any output back to the exact input, model call, and reasoning step that caused it.

### Deterministic Replay
Given an execution log, AgentStack can replay any agent run exactly — same inputs, same tool calls, same outputs. Reproduce bugs, audit decisions, validate fixes.

### Trust Propagation
Agents operate within a trust graph. Actions taken by a low-trust component cannot escalate privileges through tool calls or message passing without explicit elevation. Trust is a first-class value, not an afterthought.

---

## Quick Start

```toml
[dependencies]
agentstack = "0.2"
```

```rust
use agentstack::{Agent, Policy, TrustLevel};

#[tokio::main]
async fn main() {
    let agent = Agent::builder()
        .trust(TrustLevel::Restricted)
        .policy(Policy::strict())
        .tool(web_search)
        .tool(file_read)
        .build();

    match agent.run("analyze this codebase").await {
        Ok(result) => println!("{}", result.output),
        Err(e) => eprintln!("Interrupted: {:?}", e),
    }
}
```

---

## Architecture

```
┌─────────────────────────────────────┐
│            Agent Runtime            │
├──────────┬──────────┬───────────────┤
│  Typed   │  Causal  │     Trust     │
│Interrupts│  Tracer  │  Propagation  │
├──────────┴──────────┴───────────────┤
│         Deterministic Log           │
├─────────────────────────────────────┤
│      Tool Execution Sandbox         │
└─────────────────────────────────────┘
```

---

## Use Cases

- **Production AI agents** that need audit trails for compliance
- **Multi-agent systems** where trust between components matters
- **Agent debugging** — replay failures exactly as they occurred
- **Policy enforcement** — hard limits on what agents can do

---

## Built By

[Quantum Fabric Industries](https://github.com/QuantumFabricIndustries) — AI infrastructure, cybersecurity tooling, and audio DSP research.

---

## License

MIT

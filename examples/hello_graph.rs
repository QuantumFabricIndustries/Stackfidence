//! Minimal example: a two-node graph with a loop.
//!
//! Run: `cargo run --example hello_graph`

use agent_stack::executor::RunConfig;
use agent_stack::graph::{Edge, Graph, Node};
use agent_stack::loop_spec::LoopSpec;
use agent_stack::mock_agent::{BlackboardReader, BlackboardWriter, MockAgent};
use agent_stack::runtime::Runtime;
use agent_stack::value::Value;

fn main() {
    let rt = Runtime::permissive();
    // An agent that writes a greeting, and one that reads it back.
    rt.register(
        "greeter",
        BlackboardWriter::new("greeter", "greeting", Value::str("hello from agentstack")),
    );
    rt.register("reader", BlackboardReader::new("reader", "greeting"));

    let mut g = Graph::new("hello");
    // A loop that runs the greeter 2 times (just to show iteration).
    let a = g.add_node(
        Node::agent("greeter", "greeter")
            .with_output_slot("out.greeter")
            .with_loop(LoopSpec::repeat(2)),
    );
    let b = g.add_node(Node::agent("reader", "reader").with_output_slot("out.reader"));
    g.add_edge(Edge::normal(a, b)).unwrap();

    let out = rt.run(&g, RunConfig::default()).unwrap();
    println!("status: {:?}", out.status);
    println!("spans:  {}", out.span_count);
    println!("greeting slot = {}", rt.blackboard.read(rt.trust_root.actor, &"greeting".into()).unwrap());
    println!("reader output = {}", out.final_snapshot.get("out.reader").cloned().unwrap_or(Value::null()));

    // Also show the trivial mock agent.
    let _ = MockAgent::constant("c", Value::int(42));
}

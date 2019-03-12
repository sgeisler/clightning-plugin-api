extern crate clightning_plugin_api;
extern crate clightningrpc;
#[macro_use] extern crate serde_derive;

use clightning_plugin_api::{NoOptions, Plugin, PluginContext, RpcMethod, RpcMethodParams};
use std::sync::atomic::{AtomicBool, Ordering};

struct TestContext {
    state: AtomicBool,
}

#[derive(Deserialize)]
struct TestRequest {
    state: bool,
}

// This would be automaticcally derived
impl RpcMethodParams for TestRequest {
    fn usage() -> &'static str {
        "new_state"
    }
}


fn main() {
    // {"jsonrpc":"2.0","method":"getmanifest","id":"aloha"}
    let ctx = TestContext {
        state: AtomicBool::new(false),
    };

    let mut plugin = Plugin::<NoOptions, _>::with_context(&ctx)
        .mount_rpc(RpcMethod::new(
            "hello_world",
            "test rpc call that changes some boolean state",
            |ctx: PluginContext<NoOptions, TestContext>, request: TestRequest| {
                let old = ctx.context.state.swap(request.state, Ordering::Relaxed);
                Ok(if old != request.state {
                    "State changed!"
                } else {
                    "State didn't change."
                })
            }
        ));

    plugin.run();
}
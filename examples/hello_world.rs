extern crate clightning_plugin_api;
extern crate clightningrpc;
#[macro_use] extern crate serde_derive;

use clightningrpc::LightningRPC;
use clightning_plugin_api::{Plugin, PluginContext, RpcMethod, RpcMethodParams};
use std::path::Path;
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
        "name"
    }
}


fn main() {
    // {"jsonrpc":"2.0","method":"getmanifest","id":"aloha"}
    let ctx = TestContext {
        state: AtomicBool::new(false),
    };

    let mut plugin = Plugin::<(), _>::with_context(&ctx)
        .mount_rpc(RpcMethod::new(
            "hello_world",
            "test rpc call",
            |ctx: PluginContext<(), TestContext>, request: TestRequest| {
                ctx.context.state.store(request.state, Ordering::Relaxed);
            }
        ));

    plugin.run();
}
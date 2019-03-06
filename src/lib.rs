#![feature(fn_traits)]

extern crate clightningrpc;
extern crate serde;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;

pub use clightningrpc::LightningRPC;

use std::collections::HashMap;
use std::marker::PhantomData;

pub struct Plugin<'a, O: CmdOptions + Sync, C: Sync> {
    context: &'a C,
    rpcs: HashMap<String, &'a dyn RpcCallable<'a, O, C>>,
    plugin_state: PluginState<O>,
}

impl<'a, O, C> Plugin<'a, O, C>
    where O: CmdOptions + Sync,
          C: Sync,
{
    pub fn new() -> Plugin<'a, O, ()> {
        Plugin {
            context: &(),
            rpcs: HashMap::new(),
            plugin_state: PluginState::Starting,
        }
    }

    pub fn with_context(context: &'a C) -> Plugin<'a, O, C> {
        Plugin {
            context,
            rpcs: HashMap::new(),
            plugin_state: PluginState::Starting,
        }
    }

    fn options(&self) -> &O {
        match self.plugin_state {
            PluginState::Starting => {panic!("fetching options before init")},
            PluginState::Initialized {
                ref options,
                ..
            } => options,
        }
    }

    fn lightningd(&self) -> &LightningRPC {
        match self.plugin_state {
            PluginState::Starting => {panic!("fetching options before init")},
            PluginState::Initialized {
                ref lightningd,
                ..
            } => lightningd,
        }
    }

    pub fn mount_rpc<P, R, F>(mut self, rpc_method: &'a RpcMethod<'a, O, C, P, R, F>) -> Self
        where P: RpcMethodParams,
              R: serde::Serialize,
              F: 'a + Fn(PluginContext<'a, O, C>, P) -> R
    {
        if self.rpcs.contains_key(rpc_method.name()) {
            panic!("Tried to mount two rpc methods with the same name");
        }

        self.rpcs.insert(rpc_method.name().to_owned(), rpc_method);
        self
    }

    fn handle_init(&mut self, options: O, lightningd: LightningRPC) {
        self.plugin_state = PluginState::Initialized {
            options,
            lightningd,
        }
    }

    fn handle_rpc(&'a self, request: serde_json::Value) {
        let name = request.get("name").and_then(serde_json::Value::as_str).unwrap();
        let params = request.get("params").unwrap().to_owned();

        self.rpcs.get(name).unwrap().call(
            PluginContext {
                options: self.options(),
                lightningd: self.lightningd(),
                context: self.context
            },
            params
        );
    }
}

pub trait RpcMethodParams : serde::de::DeserializeOwned {
    fn usage() -> &'static str;
}

pub struct RpcMethod<'a, O, C, P, R, F>
    where O: 'a + CmdOptions + Sync,
          C: 'a + Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<'a, O, C>, P) -> R,
{
    name: String,
    description: String,
    action: F,
    _phantom: PhantomData<&'a (O, C, P, R)>
}

impl<'a, O, C, P, R, F> RpcMethod<'a, O, C, P, R, F>
    where O: 'a + CmdOptions + Sync,
          C: 'a + Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<'a, O, C>, P) -> R,
{
    pub fn new(name: String, description: String, action: F) -> Self {
        RpcMethod {
            name,
            description,
            action,
            _phantom: PhantomData
        }
    }
}

pub trait RpcCallable<'a, O, C>
    where O: 'a + CmdOptions + Sync,
          C: 'a + Sync,
{
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn usage(&self) -> &str;
    fn call(&self, ctx: PluginContext<'a, O, C>, params: serde_json::Value) -> serde_json::Value;
}

impl<'a, O, C, P, R, F> RpcCallable<'a, O, C> for RpcMethod<'a, O, C, P, R, F>
    where O: 'a + CmdOptions + Sync,
          C: 'a + Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<'a, O, C>, P) -> R,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn usage(&self) -> &str {
        <P as RpcMethodParams>::usage()
    }

    fn call(&self, ctx: PluginContext<'a, O, C>, params: serde_json::Value) -> serde_json::Value {
        let params: P = serde_json::from_value(params).unwrap();
        serde_json::to_value(self.action.call((ctx, params))).unwrap()
    }
}

enum PluginState<O>
    where O: CmdOptions + Sync
{
    Starting,
    Initialized {
        options: O,
        lightningd: LightningRPC,
    }
}

pub struct PluginContext<'a, O, C>
    where C: Sync,
          O: CmdOptions + Sync
{
    options: &'a O,
    lightningd: &'a LightningRPC,
    context: &'a C,
}

pub trait CmdOptions : serde::de::DeserializeOwned {
    fn options() -> &'static [CmdOptionMeta];
}

impl CmdOptions for () {
    fn options() -> &'static [CmdOptionMeta] {
        &[]
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdOptionType {
    String,
}

#[derive(Serialize)]
pub struct CmdOptionMeta {
    pub name: &'static str,
    pub option_type: CmdOptionType,
    pub default: &'static str,
    pub description: &'static str,
}

#[cfg(test)]
mod tests {
    #[test]
    fn rpc() {
        use clightningrpc::LightningRPC;
        use crate::{Plugin, PluginContext, RpcMethod, RpcMethodParams};
        use std::path::Path;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TestContext {
            state: AtomicBool,
        }

        let ctx = TestContext {
            state: AtomicBool::new(false),
        };

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

        // This could be generated by a macro, e.g. as const global (make strings static etc.)
        let rpc_method = RpcMethod::new(
            "hello_world".to_owned(),
            "test rpc call".to_owned(),
            |ctx: PluginContext<(), TestContext>, request: TestRequest| {
                ctx.context.state.store(request.state, Ordering::Relaxed);
            }
        );

        let mut plugin = Plugin::<(), _>::with_context(&ctx)
            .mount_rpc(&rpc_method);

        // Fake receiving the init response
        plugin.handle_init((), LightningRPC::new(Path::new("")));

        // Fake receiving RPC call
        plugin.handle_rpc(json!({"name": "hello_world", "params": {"state": true}}));
        assert_eq!(ctx.state.load(Ordering::Relaxed), true);

        plugin.handle_rpc(json!({"name": "hello_world", "params": {"state": false}}));
        assert_eq!(ctx.state.load(Ordering::Relaxed), false);
    }

}
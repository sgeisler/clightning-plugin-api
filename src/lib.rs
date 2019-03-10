#![feature(fn_traits)]

extern crate clightningrpc;
extern crate serde;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;

pub use clightningrpc::LightningRPC;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::io::{BufRead, BufReader, Write};

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Option<serde_json::Value>,
    id: u64,
}

#[derive(Serialize)]
struct JsonRpcResponse<T> {
    jsonrpc: String,
    result: T,
    id: u64,
}

#[derive(Serialize)]
struct ManifestResult {
    options: &'static [CmdOptionMeta],
    rpcmethods: Vec<RpcMethodMeta>,
    subscriptions: Vec<Subscription>
}

#[derive(Deserialize)]
struct InitRequest<O> {
    options: O,
    configuration: LightningdConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LightningdConfig {
    lightning_dir: String,
    rpc_file: String,
}

#[derive(Serialize)]
enum Subscription {}

pub struct Plugin<'a, O: CmdOptions + Sync, C: Sync> {
    context: &'a C,
    rpcs: HashMap<String, Box<dyn RpcCallable<O, C>>>,
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

    pub fn mount_rpc<M>(mut self, rpc_method: M) -> Self
        where M: 'static + RpcCallable<O, C>
    {
        if self.rpcs.contains_key(rpc_method.meta().name) {
            panic!("Tried to mount two rpc methods with the same name");
        }

        self.rpcs.insert(rpc_method.meta().name.to_owned(), Box::new(rpc_method));
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

    pub fn run(&mut self) {
        let mut stdin = BufReader::new(std::io::stdin());
        let mut stdout = std::io::stdout();

        loop {
            let request: JsonRpcRequest = read_obj(&mut stdin).expect("read json object");

            match request.method.as_str() {
                "getmanifest" => {
                    serde_json::to_writer(&mut stdout, &JsonRpcResponse {
                        jsonrpc: "2.0".to_owned(),
                        result: ManifestResult {
                            options: O::options(),
                            rpcmethods: self.rpcs.values().map(|rpc| rpc.meta()).collect(),
                            subscriptions: vec![]
                        },
                        id: request.id
                    }).unwrap();
                    write!(&mut stdout, "\n\n").unwrap();
                },
                "init" => {
                    let init_request: InitRequest<O> = serde_json::from_value(request.params.unwrap()).expect("error parsing init request");
                    let init_state = PluginState::Initialized {
                        options: init_request.options,
                        lightningd: LightningRPC::new(std::path::Path::new(&format!(
                                "{}/{}",
                                init_request.configuration.lightning_dir,
                                init_request.configuration.rpc_file
                        ))),
                    };
                    self.plugin_state = init_state;
                }
                method => {
                    match self.rpcs.get(method) {
                        Some(rpc_method) => {
                            let (options, lightningd) = match self.plugin_state {
                                PluginState::Starting => {
                                    eprintln!("plugin not initialized yet!");
                                    continue;
                                },
                                PluginState::Initialized {
                                    ref options,
                                    ref lightningd,
                                } => (options, lightningd),
                            };

                            let result = rpc_method.call(
                                PluginContext {
                                    options: options,
                                    lightningd: lightningd,
                                    context: self.context,
                                },
                                request.params.unwrap()
                            );

                            serde_json::to_writer(&mut stdout, &JsonRpcResponse {
                                jsonrpc: "2.0".to_string(),
                                result: result,
                                id: request.id
                            }).unwrap();
                            write!(&mut stdout, "\n\n").unwrap();
                        },
                        None => {
                            eprintln!("Unknown method: {}", method);
                            continue;
                        },
                    }
                }
            }
        }
    }
}

fn read_obj<R: BufRead, O: serde::de::DeserializeOwned>(reader: &mut R) -> serde_json::Result<O> {
    let mut obj_str = String::new();

    while obj_str.chars().all(|c| c.is_whitespace()) {
        obj_str = reader.lines()
            .map(|line| line.expect("io error"))
            .take_while(|line| line.chars().any(|c| !c.is_whitespace()))
            .collect::<Vec<_>>()
            .concat();
    }

    eprintln!("Received: {}", obj_str);
    serde_json::from_str(&obj_str)
}

pub trait RpcMethodParams : serde::de::DeserializeOwned {
    fn usage() -> &'static str;
}

pub struct RpcMethod<O, C, P, R, F>
    where O: CmdOptions + Sync,
          C: Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<O, C>, P) -> R,
{
    name: &'static str,
    description: &'static str,
    action: F,
    _phantom: PhantomData<(O, C, P, R)>
}

impl<O, C, P, R, F> RpcMethod<O, C, P, R, F>
    where O: CmdOptions + Sync,
          C: Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<O, C>, P) -> R,
{
    pub fn new(name: &'static str, description: &'static str, action: F) -> Self {
        RpcMethod {
            name,
            description,
            action,
            _phantom: PhantomData
        }
    }
}

pub trait RpcCallable<O, C>
    where O: CmdOptions + Sync,
          C: Sync,
{
    fn meta(&self) -> RpcMethodMeta;
    fn call(&self, ctx: PluginContext<O, C>, params: serde_json::Value) -> serde_json::Value;
}

impl<O, C, P, R, F> RpcCallable<O, C> for RpcMethod<O, C, P, R, F>
    where O: CmdOptions + Sync,
          C: Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<O, C>, P) -> R,
{
    fn meta(&self) -> RpcMethodMeta {
        RpcMethodMeta {
            name: self.name,
            description: self.description,
        }
    }

    fn call(&self, ctx: PluginContext<O, C>, params: serde_json::Value) -> serde_json::Value {
        let params: P = serde_json::from_value(params).unwrap();
        serde_json::to_value(self.action.call((ctx, params))).unwrap()
    }
}

#[derive(Serialize)]
pub struct RpcMethodMeta {
    name: &'static str,
    description: &'static str,
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
    pub options: &'a O,
    pub lightningd: &'a LightningRPC,
    pub context: &'a C,
}

pub trait CmdOptions : serde::de::DeserializeOwned {
    fn options() -> &'static [CmdOptionMeta];
}

pub struct NoOptions;

impl CmdOptions for NoOptions {
    fn options() -> &'static [CmdOptionMeta] {
        &[]
    }
}

impl<'de> serde::Deserialize<'de> for NoOptions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<NoOptions, D::Error> {
        Ok(NoOptions)
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

        let mut plugin = Plugin::<(), _>::with_context(&ctx)
            .mount_rpc(RpcMethod::new(
                "hello_world",
                "test rpc call",
                |ctx: PluginContext<(), TestContext>, request: TestRequest| {
                    ctx.context.state.store(request.state, Ordering::Relaxed);
                }
            ));

        // Fake receiving the init response
        plugin.handle_init((), LightningRPC::new(Path::new("")));

        // Fake receiving RPC call
        plugin.handle_rpc(json!({"name": "hello_world", "params": {"state": true}}));
        assert_eq!(ctx.state.load(Ordering::Relaxed), true);

        plugin.handle_rpc(json!({"name": "hello_world", "params": {"state": false}}));
        assert_eq!(ctx.state.load(Ordering::Relaxed), false);
    }

}
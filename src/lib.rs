#![feature(fn_traits)]

extern crate clightningrpc;
extern crate serde;
#[macro_use] extern crate serde_derive;
extern crate serde_json;

pub use clightningrpc::LightningRPC;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::io::{BufRead, BufReader, Write};

fn json_null() -> serde_json::Value {
    serde_json::Value::Null
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default = "json_null")]
    params: serde_json::Value,
    id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonRpcResult<T> {
    Result(T),
    Error(RpcError),
}

#[derive(Serialize)]
struct JsonRpcResponse<T> {
    jsonrpc: String,
    #[serde(flatten)]
    result: JsonRpcResult<T>,
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

    pub fn mount_rpc<M>(mut self, rpc_method: M) -> Self
        where M: 'static + RpcCallable<O, C>
    {
        if self.rpcs.contains_key(rpc_method.meta().name) {
            panic!("Tried to mount two rpc methods with the same name");
        }

        self.rpcs.insert(rpc_method.meta().name.to_owned(), Box::new(rpc_method));
        self
    }

    pub fn run(&mut self) {
        let mut stdin = BufReader::new(std::io::stdin());
        let mut stdout = std::io::stdout();

        loop {
            let request: JsonRpcRequest = match read_obj(&mut stdin) {
                Ok(request) => request,
                Err(e) => {
                    eprintln!("Couldn't parse request: {:?}", e);
                    continue;
                },
            };

            let result: Result<serde_json::Value, RpcError> = match request.method.as_str() {
                "getmanifest" => self.handle_getmanifest(),
                "init" => {
                    match self.handle_init(request.params) {
                        Ok(()) => {},
                        Err(e) => {
                            eprintln!("Couldn't process init message: {:?}", e);
                            // shut down, there is nothing we can do to recover
                            return;
                        },
                    }
                    // clightning doesn't expect an answer
                    continue;
                }
                method => self.handle_rpc_call(method, request.params),
            };

            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: result.into(),
                id: request.id
            };
            serde_json::to_writer(&mut stdout, &response).expect("IO error");
            write!(&mut stdout, "\n\n").expect("IO error");
        }
    }



    fn handle_getmanifest(&self) -> Result<serde_json::Value, RpcError> {
        Ok(serde_json::to_value(ManifestResult {
            options: O::options(),
            rpcmethods: self.rpcs.values().map(|rpc| rpc.meta()).collect(),
            subscriptions: vec![]
        }).expect("Serializing response failed"))
    }

    fn handle_init(&mut self, init_request: serde_json::Value) -> Result<(), RpcError> {
        let init_request: InitRequest<O> = parse_params(init_request)?;

        let init_state = PluginState::Initialized {
            options: init_request.options,
            lightningd: LightningRPC::new(std::path::Path::new(&format!(
                "{}/{}",
                init_request.configuration.lightning_dir,
                init_request.configuration.rpc_file
            ))),
        };
        self.plugin_state = init_state;

        Ok(())
    }

    fn handle_rpc_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let method = match self.rpcs.get(method) {
            Some(method) => method,
            None => return Err(RpcError {
                code: -32601,
                message: format!("Method not found: {}", method),
            }),
        };

        let (options, lightningd) = match self.plugin_state {
            PluginState::Starting => return Err(RpcError {
                code: -32603,
                message: "Plugin isn't initialized yet!".to_string()
            }),
            PluginState::Initialized {
                ref options,
                ref lightningd,
            } => (options, lightningd),
        };

        method.call(
            PluginContext {
                options: options,
                lightningd: lightningd,
                context: self.context,
            },
            params
        )
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: serde_json::Value) -> Result<T, RpcError> {
    match serde_json::from_value(params) {
        Ok(req) => Ok(req),
        Err(e) => Err(RpcError {
            code: -32602,
            message: format!("Couldn't parse params: {:?}", e),
        }),
    }
}

impl<T> From<Result<T, RpcError>> for JsonRpcResult<T> {
    fn from(res: Result<T, RpcError>) -> Self {
        match res {
            Ok(r) => JsonRpcResult::Result(r),
            Err(e) => JsonRpcResult::Error(e),
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
          F: Fn(PluginContext<O, C>, P) -> Result<R, RpcError>,
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
          F: Fn(PluginContext<O, C>, P) -> Result<R, RpcError>,
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

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

pub trait RpcCallable<O, C>
    where O: CmdOptions + Sync,
          C: Sync,
{
    fn meta(&self) -> RpcMethodMeta;
    fn call(&self, ctx: PluginContext<O, C>, params: serde_json::Value) -> Result<serde_json::Value, RpcError>;
}

impl<O, C, P, R, F> RpcCallable<O, C> for RpcMethod<O, C, P, R, F>
    where O: CmdOptions + Sync,
          C: Sync,
          P: RpcMethodParams,
          R: serde::Serialize,
          F: Fn(PluginContext<O, C>, P) -> Result<R, RpcError>,
{
    fn meta(&self) -> RpcMethodMeta {
        RpcMethodMeta {
            name: self.name,
            description: self.description,
            usage: P::usage(),
        }
    }

    fn call(&self, ctx: PluginContext<O, C>, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let params: P = parse_params(params)?;
        Ok(serde_json::to_value(
            self.action.call((ctx, params))?
        ).expect("Error while encoding response!"))
    }
}

#[derive(Serialize)]
pub struct RpcMethodMeta {
    name: &'static str,
    description: &'static str,
    usage: &'static str,
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
    fn deserialize<D: serde::Deserializer<'de>>(_deserializer: D) -> Result<NoOptions, D::Error> {
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
        use crate::{NoOptions, Plugin, PluginContext, RpcMethod, RpcMethodParams};
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

        let mut plugin = Plugin::<NoOptions, _>::with_context(&ctx)
            .mount_rpc(RpcMethod::new(
                "hello_world",
                "test rpc call",
                |ctx: PluginContext<NoOptions, TestContext>, request: TestRequest| {
                    ctx.context.state.store(request.state, Ordering::Relaxed);
                    Ok(())
                }
            ));

        // Fake receiving the init response
        plugin.handle_init(serde_json::from_str(
            "{\"options\": {}, \"configuration\": {\"lightning-dir\": \"/home/not_a_user/.lightning\", \"rpc-file\": \"lightning-rpc\"}}"
        ).unwrap()).unwrap();

        // Fake receiving RPC call
        plugin.handle_rpc_call("hello_world", serde_json::from_str(
            "{\"state\": true}"
        ).unwrap()).unwrap();
        assert_eq!(ctx.state.load(Ordering::Relaxed), true);

        plugin.handle_rpc_call("hello_world", serde_json::from_str(
            "{\"state\": false}"
        ).unwrap()).unwrap();
        assert_eq!(ctx.state.load(Ordering::Relaxed), false);
    }

}
#![feature(fn_traits)]
#![deny(missing_docs)]

//! # Plugin API for c-lightning
//!
//! This crate provides a framework to build c-lightning plugins. It abstracts away the JSON RPC
//! communication and enables you to react to different event types. Since it's still moving quickly
//! there aren't any examples in the docs, please have a look at the `examples` directory. The
//! `README.md` also contains an overview of the demonstrated features.

extern crate clightningrpc;
extern crate serde;
#[macro_use] extern crate serde_derive;
extern crate serde_json;

pub use clightningrpc::{LightningRPC, responses::NetworkAddress};

use std::collections::HashMap;
use std::marker::PhantomData;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Deserialize, Serialize)]
struct JsonRpcRequest<T> {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum JsonRpcResult<T> {
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
struct ManifestResult<> {
    options: &'static [CmdOptionMeta],
    rpcmethods: Vec<RpcMethodMeta>,
    subscriptions: Vec<String>,
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

/// Internal enum to represent event callbacks
pub enum Subscription<O: CmdOptions + Sync, C: Sync> {
    /// Connect event
    Connect(Box<dyn Fn(PluginContext<O, C>, ConnectEvent)>),
    /// Disconnect event
    Disconnect(Box<dyn Fn(PluginContext<O, C>, DisconnectEvent)>),
}

/// Internal trait for event closure arguments
pub trait Event {
    /// Convert user-provided closure to "type-erased" `Subscription` enum
    fn subscription<O, C, F>(event_fn: F) -> Subscription<O, C>
        where O: 'static + CmdOptions + Sync,
              C: 'static + Sync,
              F: 'static + Fn(PluginContext<O, C>, Self),
              Self: Sized;

    /// event name as c-lightning sends it
    fn name() -> &'static str;
}

/// Data received when we get a new connection to another node
#[derive(Deserialize)]
pub struct ConnectEvent {
    /// Id of the newly connected node
    pub id: String,

    /// Network address of the newly connected node
    pub address: NetworkAddress,
}

impl Event for ConnectEvent {
    fn subscription<O, C, F>(event_fn: F) -> Subscription<O, C>
        where O: 'static + CmdOptions + Sync,
              C: 'static + Sync,
              F: 'static + Fn(PluginContext<O, C>, Self)
    {
        Subscription::Connect(Box::new(event_fn))
    }

    fn name() -> &'static str {
        "connect"
    }
}

#[derive(Deserialize)]
/// Data received when a node disconnects from us
pub struct DisconnectEvent {
    /// Id of the disconnected node
    pub id: String,
}

impl Event for DisconnectEvent {
    fn subscription<'a, O, C, F>(event_fn: F) -> Subscription<O, C>
        where O: 'static + CmdOptions + Sync,
              C: 'static + Sync,
              F: 'static + Fn(PluginContext<O, C>, Self)
    {
        Subscription::Disconnect(Box::new(event_fn))
    }

    fn name() -> &'static str {
        "disconnect"
    }
}

/// Main plugin structure to register RPC methods, options and events with
pub struct Plugin<O: CmdOptions + Sync, C: Sync> {
    context: Arc<C>,
    rpcs: HashMap<String, Box<dyn RpcCallable<O, C>>>,
    subscriptions: HashMap<String, Subscription<O, C>>,
    plugin_state: PluginState<O>,
    log_channel: (Sender<LogMessage>, Receiver<LogMessage>),
}

impl<O, C> Plugin<O, C>
    where O: 'static + CmdOptions + Sync,
          C: 'static + Sync,
{
    /// Initialize plugin without global state (without context)
    pub fn new() -> Plugin<O, ()> {
        Plugin {
            context: Arc::new(()),
            rpcs: HashMap::new(),
            subscriptions: HashMap::new(),
            plugin_state: PluginState::Starting,
            log_channel: channel(),
        }
    }

    /// Initialize plugin with global state (context)
    pub fn with_context(context: Arc<C>) -> Plugin<O, C> {
        Plugin {
            context,
            rpcs: HashMap::new(),
            subscriptions: HashMap::new(),
            plugin_state: PluginState::Starting,
            log_channel: channel(),
        }
    }

    /// Register an RPC method that will be exposed to the user
    pub fn mount_rpc<M>(mut self, rpc_method: M) -> Self
        where M: 'static + RpcCallable<O, C>
    {
        if self.rpcs.contains_key(rpc_method.meta().name) {
            panic!("Tried to mount two rpc methods with the same name");
        }

        self.rpcs.insert(rpc_method.meta().name.to_owned(), Box::new(rpc_method));
        self
    }

    /// Subscribe to an event type by specifying a closure that takes a `PluginContext` and an
    /// event-specific data structure (implementing `Event`) as arguments.
    pub fn subscribe<F, E>(mut self, event_fn: F) -> Self
        where E: Event,
              F: 'static + Fn(PluginContext<O, C>, E),
    {
        let subscription = E::subscription(event_fn);
        let event_name = E::name();

        if self.subscriptions.contains_key(event_name) {
            panic!("Tried to subscribe to the same event twice.");
        }

        self.subscriptions.insert(event_name.to_owned(), subscription);
        self
    }

    /// Run the plugin till it gets killed by lightningd
    pub fn run(&mut self) {
        let mut stdin = BufReader::new(std::io::stdin());
        let mut stdout = std::io::stdout();

        loop {
            // Print all log messages from previous round
            for log_message in self.log_channel.1.try_iter() {
                serde_json::to_writer(
                    &mut stdout,
                    &JsonRpcRequest {
                        jsonrpc: "2.0".to_string(),
                        method: "log".to_string(),
                        params: log_message,
                        id: None
                    }
                ).expect("IO error");
                write!(&mut stdout, "\n\n").expect("IO error");
            }

            let request: JsonRpcRequest<serde_json::Value> = match read_obj(&mut stdin) {
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
                        Ok(()) => {
                            // clightning doesn't expect an answer
                            continue;
                        },
                        Err(e) => {
                            eprintln!("Couldn't process init message: {:?}", e);
                            // shut down, there is nothing we can do to recover
                            return;
                        },
                    }
                },
                event if self.subscriptions.contains_key(event) => {
                    match self.handle_event(event, request.params) {
                        Ok(()) => {},
                        Err(e) => {
                            eprintln!("Couldn't process event '{}': {:?}", event, e);
                        },
                    };
                    continue;
                },
                method => self.handle_rpc_call(method, request.params),
            };

            let response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: result.into(),
                id: request.id.expect("this request should have an ID"),
            };
            serde_json::to_writer(&mut stdout, &response).expect("IO error");
            write!(&mut stdout, "\n\n").expect("IO error");
        }
    }

    fn handle_event(&self, event: &str, params: serde_json::Value) -> Result<(), RpcError> {
        match self.subscriptions.get(event) {
            Some(Subscription::Connect(f)) => f.call((
                self.get_plugin_context()?,
                parse_params(params)?,
                )),
            Some(Subscription::Disconnect(f)) => f.call((
                self.get_plugin_context()?,
                parse_params(params)?,
            )),
            None => return Err(RpcError {
                code: 0,
                message: "unknown event".to_string()
            }),
        }
        Ok(())
    }

    fn handle_getmanifest(&self) -> Result<serde_json::Value, RpcError> {
        Ok(serde_json::to_value(ManifestResult {
            options: O::options(),
            rpcmethods: self.rpcs.values().map(|rpc| rpc.meta()).collect(),
            subscriptions: self.subscriptions.keys().cloned().collect(),
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

        method.call(
            self.get_plugin_context()?,
            params
        )
    }

    fn get_plugin_context<'a>(&'a self) -> Result<PluginContext<'a, O, C>, RpcError> {
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

        Ok(PluginContext {
            options,
            lightningd,
            context: self.context.as_ref(),
            log_sender: self.log_channel.0.clone(),
        })
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

/// Trait that extracts meta data about RPC method parameter structs
pub trait RpcMethodParams : serde::de::DeserializeOwned {
    /// How to use the RPC method, e.g. `'arg1 [arg2]'` for one non-optional and one optional
    /// argument
    fn usage() -> &'static str;
}

/// Type erased reference to a RPC method callback with some additional metadata
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
    /// Create a new type-erased RPC method callback object with additional meta data to be
    /// displayed on the help page of `lightning-cli`
    pub fn new(name: &'static str, description: &'static str, action: F) -> Self {
        RpcMethod {
            name,
            description,
            action,
            _phantom: PhantomData
        }
    }
}

/// Error that can happen during RPC method execution
#[derive(Debug, Serialize)]
pub struct RpcError {
    /// JSON RPC error code
    pub code: i64,

    /// Custom message
    pub message: String,
}

/// Internal trait for `RpcMethod`
pub trait RpcCallable<O, C>
    where O: CmdOptions + Sync,
          C: Sync,
{
    /// Get metadata
    fn meta(&self) -> RpcMethodMeta;

    /// Invoke callback
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

/// RPC method meta data displayed as help to the user
#[derive(Serialize)]
pub struct RpcMethodMeta {
    /// RPC method's name
    name: &'static str,

    /// Description of what the RPC method does
    description: &'static str,

    /// Argument list
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

/// Log level of log message
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum LogLevel {
    Io,
    Debug,
    Info,
    Unusual,
    Broken,
}

#[derive(Serialize)]
struct LogMessage {
    pub level: LogLevel,
    pub message: String,
}

/// Plugin state
pub struct PluginContext<'a, O, C>
    where C: Sync,
          O: CmdOptions + Sync
{
    /// Options received from c-lightning
    pub options: &'a O,

    /// RPC client connected to lightningd
    pub lightningd: &'a LightningRPC,

    /// User defined global state
    pub context: &'a C,
    log_sender: Sender<LogMessage>
}

impl<'a, O, C> PluginContext<'a, O, C>
    where C: Sync,
          O: CmdOptions + Sync
{
    /// Log a message to lightningd's log output
    pub fn log(&self, level: LogLevel, message: String) {
        self.log_sender.send(LogMessage {
            level,
            message,
        }).expect("main thread died?");
    }
}

/// Trait that gives meta information about command line option structs
pub trait CmdOptions : serde::de::DeserializeOwned {
    /// Command line options
    fn options() -> &'static [CmdOptionMeta];
}

/// If you don't want to have any options, use this (`()` doesn't deserialize correctly)
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

/// Option data types supported by c-lightning
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CmdOptionType {
    /// String
    String,
}

/// Meta data about a command line option
#[derive(Serialize)]
pub struct CmdOptionMeta {
    /// Command line option name (`--name`)
    pub name: &'static str,

    /// Expected data type
    #[serde(rename = "type")]
    pub option_type: CmdOptionType,

    /// Default if no input is provided
    pub default: &'static str,

    /// Description of the command line option
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
extern crate clightning_plugin_api;
extern crate clightningrpc;
#[macro_use] extern crate serde_derive;

use clightning_plugin_api::{CmdOptionMeta, CmdOptionType, CmdOptions, Plugin, PluginContext, RpcMethod, RpcMethodParams};

#[derive(Debug, Eq, PartialEq)]
struct HelloWorldRequest {
    name: Option<String>,
    message: Option<String>,
}

impl<'de> serde::Deserialize<'de> for HelloWorldRequest {
    fn deserialize<D>(deserializer: D) -> Result<HelloWorldRequest, D::Error>
        where D: serde::Deserializer<'de>
    {
        // Should handle wrong number of arguments error better
        let params: Vec<String> = serde::Deserialize::deserialize(deserializer)?;
        let mut param_iter = params.into_iter();
        Ok(HelloWorldRequest {
            name: param_iter.next(),
            message: param_iter.next()
        })
    }
}

// This would be automaticcally derived
impl RpcMethodParams for HelloWorldRequest {
    fn usage() -> &'static str {
        "[name] [message]"
    }
}

#[derive(Deserialize)]
struct HelloWorldOptions {
    default_name: String,
}

impl CmdOptions for HelloWorldOptions {
    fn options() -> &'static [CmdOptionMeta] {
        &[
            CmdOptionMeta {
                name: "default_name",
                option_type: CmdOptionType::String,
                default: "World",
                description: "default name used when invoking `hello_world` without a name parameter",
            }
        ]
    }
}


fn main() {
    let mut plugin = Plugin::<HelloWorldOptions, ()>::new()
        .mount_rpc(RpcMethod::new(
            "hello_world",
            "test rpc call that changes some boolean state",
            |ctx: PluginContext<HelloWorldOptions, ()>, request: HelloWorldRequest| {
                let mut response = String::new();

                response += &format!(
                    "Hello {}!",
                    request.name.unwrap_or(ctx.options.default_name.clone())
                );

                if let Some(message) = request.message {
                    response += &format!(" Your message: {}", message);
                }

                Ok(response)
            }
        ));

    plugin.run();
}

#[cfg(test)]
mod tests {
    use super::HelloWorldRequest;

    #[test]
    fn de_option() {
        let json = "[\"test\", \"test2\"]";
        let parsed: HelloWorldRequest = serde_json::from_str(json).unwrap();
        assert_eq!(HelloWorldRequest {name: Some("test".into()), message: Some("test2".into())}, parsed);

        let json = "[\"test\"]";
        let parsed: HelloWorldRequest = serde_json::from_str(json).unwrap();
        assert_eq!(HelloWorldRequest {name: Some("test".into()), message: None}, parsed);

        let json = "[]";
        let parsed: HelloWorldRequest = serde_json::from_str(json).unwrap();
        assert_eq!(HelloWorldRequest {name: None, message: None}, parsed);
    }
}
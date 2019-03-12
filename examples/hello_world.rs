extern crate clightning_plugin_api;
extern crate clightningrpc;
extern crate serde_derive;

use clightning_plugin_api::{NoOptions, Plugin, PluginContext, RpcMethod, RpcMethodParams};

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


fn main() {
    let mut plugin = Plugin::<NoOptions, ()>::new()
        .mount_rpc(RpcMethod::new(
            "hello_world",
            "test rpc call that changes some boolean state",
            |_ctx: PluginContext<NoOptions, ()>, request: HelloWorldRequest| {
                let mut response = String::new();

                response += &format!(
                    "Hello {}!",
                    request.name.unwrap_or("World".into())
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
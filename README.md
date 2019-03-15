# Rust API for c-lightning plugins [![Build Status](https://travis-ci.org/sgeisler/clightning-plugin-api.svg?branch=master)](https://travis-ci.org/sgeisler/clightning-plugin-api)

This is my first attempt at building a strongly typed, yet ergonomic API for the [c-lightning plugin API](https://github.com/ElementsProject/lightning/blob/master/doc/PLUGINS.md).

Implemented features:
* Command line options
* Custom RPC methods
* Event subscriptions

Missing features:
* Hooks
* Non-blocking RPC call/event execution
* Derive macros for `CmdOptions` and `RpcMethodParams` traits and maybe a custom `DeserializeOptional` trait for
RPC methods with optional parameters
* Human readable error reporting

# Usage

The [examples](https://github.com/sgeisler/clightning-plugin-api/tree/master/examples) show how to use the following features:

|                               | [hello_world](https://github.com/sgeisler/clightning-plugin-api/blob/master/examples/hello_world.rs) | [state](https://github.com/sgeisler/clightning-plugin-api/blob/master/examples/state.rs) | [subscription](https://github.com/sgeisler/clightning-plugin-api/blob/master/examples/subscription.rs) |
|-------------------------------|-------------|-------|--------------|
| Command line options          | ✓           |       |              |
| RPC methods                   | ✓           | ✓     |              |
| Optional RPC method arguments | ✓           |       |              |
| State                         |             | ✓     |              |
| Event subscriptions           |             |       | ✓            |
| Logging                       |             |       | ✓            | 

You can test these plugins yourself by running `lightningd --testnet --plugin target/debug/examples/<example_name>`
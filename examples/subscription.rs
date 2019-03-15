extern crate clightning_plugin_api;

use clightning_plugin_api::{ConnectEvent, DisconnectEvent, LogLevel, NoOptions, Plugin};


fn main() {
    let mut plugin = Plugin::<NoOptions, ()>::new()
        .subscribe(|ctx, event: ConnectEvent| {
            ctx.log(
                LogLevel::Info,
                format!("New connection from {}!", event.id)
            );
        })
        .subscribe(|ctx, event: DisconnectEvent| {
            ctx.log(
                LogLevel::Info,
                format!("Lost connection to {}!", event.id)
            );
        });

    plugin.run();
}